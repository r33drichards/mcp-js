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

policy_dir=$(mktemp -d)
trap 'rm -rf "$policy_dir"' EXIT

cat >"$policy_dir/filesystem.rego" <<'REGO'
package mcp.filesystem

default allow := false

allow if {
    input.operation in {"readFile", "writeFile", "mkdir", "readdir"}
    startswith(input.path, "/workspace/")
    not ".." in split(input.path, "/")
}

allow if {
    input.operation == "readdir"
    input.path == "/workspace"
}
REGO

cat >"$policy_dir/fetch.rego" <<'REGO'
package mcp.fetch

default allow := false

allow if {
    input.method == "GET"
    input.url_parsed.scheme == "https"
    input.url_parsed.host == "example.com"
    input.url_parsed.port == null
}
REGO

cat >"$policy_dir/modules.rego" <<'REGO'
package mcp.modules

default allow := false

allow if {
    input.resolved_url == "https://cdn.jsdelivr.net/npm/escape-html@1.0.3/+esm"
}
REGO

policies='{
  "filesystem": {"policies": [{"url": "file:///policies/filesystem.rego"}]},
  "fetch": {"policies": [{"url": "file:///policies/fetch.rego"}]},
  "modules": {"policies": [{"url": "file:///policies/modules.rego"}]}
}'

python - <<'PYTHON'
import modal

modal.Volume.objects.create("mcp-js-interpreter-policies", allow_existing=True)
PYTHON

for name in filesystem fetch modules; do
  modal volume put --force mcp-js-interpreter-policies "$policy_dir/$name.rego" "/$name.rego"
done

modal server deploy \
  --app-name mcp-js-code-interpreter \
  --image docker.io/wholelottahoopla/mcp-js:0.22.0 \
  --port 8080 \
  --min-containers 0 \
  --max-containers 1 \
  --scaledown-window 60 \
  --volume mcp-js-interpreter-state:/data \
  --volume mcp-js-interpreter-workspace:/workspace \
  --volume mcp-js-interpreter-policies:/policies \
  --env-var "MCP_V8_POLICIES_JSON=$policies" \
  --env-var MCP_V8_ALLOW_EXTERNAL_MODULES=true \
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
directly, so the container only needs the published image. The script also
uploads three Rego policy files using the local Modal CLI. The server
evaluates them in-process, so there is no separate OPA service to deploy.

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

## Read and write the workspace volume

The deploy script mounts `mcp-js-interpreter-workspace` at `/workspace`.
The separate `/data` volume holds interpreter state, while `/policies`
holds the Rego files. `MCP_V8_POLICIES_JSON` maps each capability to the
file that controls it.

Mounting a volume makes it visible to the server process. The
`mcp.filesystem` policy lets JavaScript list directories, create
directories, and read or write files anywhere under `/workspace`,
including subdirectories. Each `fs` call is checked before the server
touches the filesystem.

```sh
python run_code.py 'await fs.mkdir("/workspace/reports", {recursive: true})'
python run_code.py 'await fs.writeFile("/workspace/reports/summary.txt", "hello from Modal")'
python run_code.py 'console.log(await fs.readdir("/workspace"))'
python run_code.py 'console.log(await fs.readFile("/workspace/reports/summary.txt", "utf8"))'
```

The directory listing includes `reports`, and the final command prints
`hello from Modal`. The rule uses the `/workspace/` prefix so a sibling
such as `/workspace-other` stays outside the grant. It also rejects any
`..` path component. Rego checks the supplied path before filesystem access,
and the OS can still follow symlinks. Keep the dedicated workspace volume
free of symlinks that point outside it. See
[filesystem policy inputs and path handling](../concepts/filesystem.md).

## Fetch data and import a module

The `mcp.fetch` rule permits HTTPS GET requests to `example.com` on its
default port. Changing the method to POST or the host to another domain
causes a policy denial.

```sh
python run_code.py 'console.log((await fetch("https://example.com/")).status)'
```

To load an external module, the deployment enables
`MCP_V8_ALLOW_EXTERNAL_MODULES` and the `mcp.modules` policy permits one
exact URL. The module loader checks this separately from `fetch()`.

```sh
python run_code.py 'import escape from "https://cdn.jsdelivr.net/npm/escape-html@1.0.3/+esm"; console.log(escape("<tag>"))'
```

The import example prints `&lt;tag&gt;`. To grant another capability, add
its Rego file to the deployment script’s upload list and register it under
the matching key in `policies`. For input fields and recipes for other
capabilities, see [Security policies](../how-to/policies.md).

After editing a policy, rerun the deployment script and confirm the
redeploy. A newly started server loads the uploaded rules. Keep the
policy files on their separate volume so the workspace rule cannot modify
them.

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
`modal app stop mcp-js-code-interpreter`. The volumes remain available
for a later deployment.
