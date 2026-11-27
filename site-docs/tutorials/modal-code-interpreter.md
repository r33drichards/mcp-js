# Run a stateful JavaScript interpreter on Modal

Start a JavaScript interpreter in one terminal and send it code from
another. Modal runs the published `mcp-js` Docker image, which contains the
HTTP server and V8 runtime. Python runs locally to start the Sandbox and
send requests. Modal pulls the image for you.

## 1. Start the interpreter

In the first terminal, install the Modal SDK and sign in. You need a Modal
account and Python 3.10 or later.

```sh
python -m venv .venv
source .venv/bin/activate
python -m pip install 'modal>=1.6.4'
modal setup
```

Save this script as `deploy_interpreter.py`.

```python
"""Start the published mcp-js image in a Modal Sandbox."""

from shlex import quote

import modal

IMAGE = "docker.io/wholelottahoopla/mcp-js:0.22.0"
PORT = 8080


def main():
    app = modal.App.lookup("mcp-js-code-interpreter", create_if_missing=True)
    sandbox = modal.Sandbox.create(
        app=app,
        image=modal.Image.from_registry(IMAGE),
        env={
            "PORT": str(PORT),
            "MCP_V8_HEAP_STORE": "dir",
            "MCP_V8_HEAP_DIR": "/data/heaps",
            "MCP_V8_SESSION_DB_PATH": "/data/sessions",
        },
        readiness_probe=modal.Probe.with_tcp(PORT),
        block_network=True,
        timeout=1800,
    )
    try:
        sandbox.wait_until_ready()
        creds = sandbox.create_connect_token(port=PORT)
        print("Copy these exports into the client terminal:", flush=True)
        print(f"export MCP_JS_URL={quote(creds.url)}", flush=True)
        print(f"export MCP_JS_TOKEN={quote(creds.token)}", flush=True)
        print("Press Ctrl+C here to stop the interpreter.", flush=True)
        sandbox.wait()
    except KeyboardInterrupt:
        pass
    finally:
        sandbox.terminate()


if __name__ == "__main__":
    main()
```

Run it and leave this terminal open.

```sh
python deploy_interpreter.py
```

When the listener is ready, the launcher prints two `export` commands.
The URL points to this running Sandbox and the token authorizes HTTP
requests to it. Keep the token private.

## 2. Run JavaScript

Open a second terminal in the same directory and paste the two `export`
commands from the launcher. Save the following as `run_code.py`. This
script uses only the Python standard library.

```python
"""Submit JavaScript to a running Modal interpreter."""

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
    with urlopen(Request(base_url.rstrip("/") + path, data, headers), timeout=10) as response:
        return json.load(response)


def run_js(call, code, *, session="tutorial"):
    execution = call(
        "/api/exec",
        {"code": code, "session": session, "execution_timeout_secs": 5},
    )
    path = f"/api/executions/{execution['execution_id']}"
    deadline = time.monotonic() + 20
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
        parser.error("Set MCP_JS_URL and MCP_JS_TOKEN from the launcher output.")

    call = partial(request, url, token)
    try:
        run_js(call, args.code, session=args.session)
    except (RuntimeError, URLError, TimeoutError) as error:
        parser.exit(1, f"{error}\n")


if __name__ == "__main__":
    main()
```

Run three separate commands. Each sends JavaScript to the same named
session, even though each invocation starts a new local Python process.

```sh
python run_code.py 'globalThis.x = 10'
python run_code.py 'globalThis.y = 5'
python run_code.py 'console.log(x + y)'
```

The first two commands produce no console output. The third prints `15`.
A different session starts with an empty heap, so this command exits with
a reference error.

```sh
python run_code.py --session fresh 'console.log(x + y)'
```

Press Ctrl+C in the launch terminal when you're done. The launcher
terminates the Sandbox and its saved state is deleted. The Sandbox also
expires after 30 minutes.

## How it works

1. **The launcher starts the image.** The image entrypoint runs
   `mcp-v8` on port 8080. Once the TCP readiness probe passes, the launcher
   requests a URL and bearer token. Modal checks the token before forwarding
   each request. The Sandbox blocks outbound network access. See
   [Modal Sandbox Connect Tokens](https://modal.com/docs/guide/sandbox-networking#connecting-to-sandboxes-with-http-and-websockets).
2. **The client runs one snippet.** `POST /api/exec` queues the snippet and
   returns an execution ID. The client polls `GET /api/executions/{id}`,
   then reads `GET /api/executions/{id}/output` for the captured console
   output. If execution fails, the client prints the error and exits with
   status 1. This example reads one page of output. See the
   [HTTP API reference](../reference/http-api.md) for pagination.
3. **The server saves state by session name.** After a successful run, the
   directory heap store saves the V8 heap under `/data/heaps`. The session
   records under `/data/sessions` identify which snapshot to restore on the
   next call. That is how the third command reads `x` and `y`.

This follows the same stateful pattern as
[Modal’s Python interpreter example](https://modal.com/docs/examples/simple_code_interpreter).
The server replaces the Python worker, and the named V8 heap replaces its
shared globals dictionary.
