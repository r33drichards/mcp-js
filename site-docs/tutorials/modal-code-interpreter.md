# Run a stateful JavaScript interpreter on Modal

Deploy the published `mcp-js` Docker image once, then submit JavaScript
whenever you need it. Use the same terminal for both steps. When the
interpreter is idle, Modal can stop its container while keeping the endpoint
available. The next request starts the container again.

## 1. Deploy the interpreter

You need a Modal account and Python 3.10 or later for the local CLI. Install
the CLI and sign in.

```sh
python -m venv .venv
source .venv/bin/activate
python -m pip install 'modal>=1.6.4'
modal setup
```

Save the deployment script as `deploy_interpreter.sh`.

```bash
#!/usr/bin/env bash
set -euo pipefail

modal server deploy \
  --app-name mcp-js-code-interpreter \
  --image docker.io/wholelottahoopla/mcp-js:0.22.0 \
  --port 8080 \
  --min-containers 0 \
  --max-containers 1 \
  --scaledown-window 60 \
  --volume mcp-js-interpreter-state:/data \
  --env-var PORT=8080 \
  --env-var MCP_V8_HEAP_STORE=dir \
  --env-var MCP_V8_HEAP_DIR=/data/heaps \
  --env-var MCP_V8_SESSION_DB_PATH=/data/sessions
```

```sh
bash deploy_interpreter.sh
```

Modal prints an HTTPS URL and the deploy command exits. Copy that URL into
`MCP_JS_URL` in the next step. Modal runs the stock image’s entrypoint
directly, so the container only needs the published image.

## 2. Run JavaScript

In the same terminal, create a
[Modal Proxy Token](https://modal.com/docs/guide/webhook-proxy-auth) for the
HTTP endpoint. You can reuse one you already have for the environment.

```sh
modal workspace proxy-tokens create --name mcp-js-interpreter
```

Set the URL from deployment and combine the proxy token ID and secret with
a dot between them. Keep the token private.

```sh
export MCP_JS_URL='https://YOUR-DEPLOYED-URL.modal.run'
read -rs -p 'Modal proxy token (wk-ID.ws-SECRET): ' MCP_JS_TOKEN
echo
export MCP_JS_TOKEN
```

Save this client as `run_code.py`. It uses the Python standard library.

```python
"""Submit JavaScript to a deployed Modal interpreter."""

import argparse
import json
import os
import time
from functools import partial
from urllib.error import URLError
from urllib.request import Request, urlopen


def request(base_url, token, path, body=None):
    headers = {"Authorization": f"Bearer {token}"}
    data = None
    if body is not None:
        headers["Content-Type"] = "application/json"
        data = json.dumps(body).encode()
    with urlopen(Request(base_url.rstrip("/") + path, data, headers), timeout=60) as response:
        return json.load(response)


def run_js(call, code, *, session="tutorial"):
    execution = call(
        "/api/exec",
        {"code": code, "session": session, "execution_timeout_secs": 5},
    )
    path = f"/api/executions/{execution['execution_id']}"
    deadline = time.monotonic() + 90
    while time.monotonic() < deadline:
        result = call(path)
        status = result["status"].lower()
        if status in {"completed", "failed", "timed_out", "cancelled"}:
            output = call(f"{path}/output")["data"]
            if output:
                print(output.rstrip())
            if status != "completed" or result.get("error"):
                raise RuntimeError(result.get("error") or status)
            return
        time.sleep(0.25)
    raise TimeoutError("The execution did not finish before the client deadline")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("code", help="JavaScript source")
    parser.add_argument("--session", default="tutorial", help="Session name")
    args = parser.parse_args()

    url = os.environ.get("MCP_JS_URL")
    token = os.environ.get("MCP_JS_TOKEN")
    if not url or not token:
        parser.error("Set MCP_JS_URL to the deployed URL and MCP_JS_TOKEN to wk-ID.ws-SECRET.")

    call = partial(request, url, token)
    try:
        run_js(call, args.code, session=args.session)
    except (RuntimeError, URLError, TimeoutError) as error:
        parser.exit(1, f"{error}\n")


if __name__ == "__main__":
    main()
```

Run three separate commands. The first request might take longer while
Modal starts the container.

```sh
python run_code.py 'globalThis.x = 10'
python run_code.py 'globalThis.y = 5'
python run_code.py 'console.log(x + y)'
```

The first two commands produce no output. The third prints `15`. A new
session has its own heap, so this command exits with a reference error.

```sh
python run_code.py --session fresh 'console.log(x + y)'
```

## What happens when the app goes idle

- **The endpoint stays deployed.** `--min-containers 0` lets Modal stop the
  container after roughly 60 seconds without requests. An authenticated
  request starts a new container when needed. `--max-containers 1` keeps
  one server process writing to the state volume at a time. See
  [Modal Servers](https://modal.com/docs/guide/servers).
- **The named session lives on the volume.** The directory heap store writes
  V8 snapshots to `/data/heaps` and saves session records under
  `/data/sessions`. Both directories are on the mounted Modal Volume. Modal
  commits the volume in the background and on normal shutdown, so the
  next container can restore committed state. A sudden interruption can
  lose writes since the last commit. To try a cold start, wait for the
  deployment to show zero containers and rerun the third command. See
  [Modal Volumes](https://modal.com/docs/guide/volumes).
- **The client talks to the interpreter over HTTP.** The server queues
  each snippet on `POST /api/exec`. The client uses the returned
  execution ID to poll status and read captured console output. The
  `tutorial` session name links the three commands. See the
  [HTTP API reference](../reference/http-api.md).

To remove the endpoint when you’re done, run
`modal app stop mcp-js-code-interpreter`. The state volume remains available
for a later deployment.
