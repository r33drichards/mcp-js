# Run a stateful JavaScript interpreter on Modal

Send three separate JavaScript snippets to one Modal Sandbox. The first two
store `x` and `y`. The third reads both and prints `15`.

**Your machine runs the Python client. Modal runs the interpreter.**
The client asks Modal to start the published `mcp-js` Docker image. The image
contains the `mcp-v8` server, which receives snippets over HTTP and runs them
in V8. You only need a Modal account and Python 3.10 or later on your
machine. Modal pulls the image for you.

## Run it

Install the Modal SDK and sign in.

```sh
python -m venv .venv
source .venv/bin/activate
python -m pip install 'modal>=1.6.4'
modal setup
```

Save this complete script as `code_interpreter.py`.

```python
"""Run a stateful JavaScript interpreter from a stock image on Modal."""

import json
import time
from functools import partial
from urllib.request import Request, urlopen

import modal

IMAGE = "docker.io/wholelottahoopla/mcp-js:0.22.0"
PORT = 8080


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
        timeout=180,
    )
    try:
        sandbox.wait_until_ready()
        creds = sandbox.create_connect_token(port=PORT)
        call = partial(request, creds.url, creds.token)
        run_js(call, "globalThis.x = 10")
        run_js(call, "globalThis.y = 5")
        run_js(call, "console.log(x + y)")
        try:
            run_js(call, 'throw new Error("demo error")')
        except RuntimeError as error:
            if "demo error" not in str(error):
                raise
            print(error)
        else:
            raise AssertionError("Expected the JavaScript error")
    finally:
        sandbox.terminate()


if __name__ == "__main__":
    main()
```

Run it from the same terminal.

```sh
python code_interpreter.py
```

After the Sandbox starts, look for these two lines. The second comes from
the final snippet, which deliberately throws.

```text
15
Uncaught (in promise) Error: demo error
```

## How it works

1. **Start the image.** `main()` creates a Sandbox from the pinned
   `mcp-js` image. Its entrypoint starts the HTTP server on port 8080.
   After the TCP readiness probe passes, the client asks Modal for a URL
   and bearer token. Modal checks the token before forwarding each
   request. The Sandbox blocks outbound network access. See
   [Modal Sandbox Connect Tokens](https://modal.com/docs/guide/sandbox-networking#connecting-to-sandboxes-with-http-and-websockets).
2. **Submit code and collect the result.** `run_js()` posts the snippet
   to `/api/exec` and gets an execution ID. It polls
   `/api/executions/{id}` until the run finishes, then reads
   `/api/executions/{id}/output` for anything written to the console. If
   the execution failed, it raises with the error from the status response.
   See the [HTTP API reference](../reference/http-api.md) for response fields
   and output pagination.
3. **Reuse state by session name.** All four calls use the session
   named `tutorial`. The directory heap store saves the V8 heap after a
   successful run and the session database tracks which snapshot to load
   next. Each run finishes before the next begins. That is how the third
   snippet sees the variables assigned in the first two.
4. **Clean up.** The `finally` block terminates the Sandbox. Its local
   snapshots under `/data/heaps` and session records under `/data/sessions`
   disappear with it. Running the script again starts with fresh state.

To see what a new session can read, change the third call to
`run_js(call, "console.log(x + y)", session="fresh")` and rerun. The read
raises a reference error because that session has no saved `x` or `y`.

This follows the same stateful pattern as
[Modal’s Python interpreter example](https://modal.com/docs/examples/simple_code_interpreter).
The server replaces the Python worker, and the named V8 heap replaces its
shared globals dictionary.
