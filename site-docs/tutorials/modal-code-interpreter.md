# Run a stateful JavaScript interpreter on Modal

Create a Modal Sandbox, send it three separate JavaScript snippets, and see
the third read variables set by the first two. Then send a snippet that
throws and read the error back.

In [Modal’s original interpreter example](https://modal.com/docs/examples/simple_code_interpreter),
a Python worker reads JSON from standard input and keeps variables in a
shared globals dictionary. Here the published `mcp-js` Docker image provides
the worker. Its Rust `mcp-v8` server receives JavaScript over HTTP, executes
it inside a V8 isolate, and saves the isolate’s state under a session name.

You run the Python client on your machine. Modal pulls the pinned image into
the Sandbox and starts its entrypoint. The Sandbox needs no Python runtime,
custom Dockerfile, or additional service.

## 1. Set up the client

You need a Modal account and Python 3.10 or later. Set up the SDK from a
terminal. Local Docker is optional because Modal pulls the image itself.

```sh
python -m venv .venv
source .venv/bin/activate
python -m pip install 'modal>=1.6.4'
modal setup
```

Create a file named `code_interpreter.py` and paste the Python blocks from
steps 1–4 into it in order. The Modal SDK is the only package the script
imports outside the Python standard library.

```python
"""Run a stateful JavaScript interpreter from a stock image on Modal."""

import json
import time
from functools import partial
from urllib.request import Request, urlopen

import modal

IMAGE = "docker.io/wholelottahoopla/mcp-js:0.22.0"
PORT = 8080
```

## 2. Send an authenticated HTTP request

A [Modal Sandbox Connect Token](https://modal.com/docs/guide/sandbox-networking#connecting-to-sandboxes-with-http-and-websockets)
gives the client a URL and a bearer token for the server on port 8080. Modal
checks that token before forwarding the request. Keep both values in the
client. The JavaScript snippets received by the interpreter are just the
`code` strings you submit.

The helper sends a JSON POST when you pass a body and a GET when you leave
it out. It parses the response as JSON. An HTTP error raises in the client
before any response is passed to the JavaScript execution logic.

```python
def request(base_url, token, path, body=None):
    headers = {"Authorization": f"Bearer {token}"}
    data = None
    if body is not None:
        headers["Content-Type"] = "application/json"
        data = json.dumps(body).encode()
    with urlopen(Request(base_url.rstrip("/") + path, data, headers), timeout=10) as response:
        return json.load(response)
```

## 3. Submit a snippet and read its result

One call to `run_js` makes three kinds of requests.

| Request | What comes back |
| --- | --- |
| `POST /api/exec` | HTTP 202 and an `execution_id` for the queued snippet. |
| `GET /api/executions/{id}` | The execution’s status and any JavaScript error. |
| `GET /api/executions/{id}/output` | Captured console output in the `data` field. |

Each call in the demo uses the default session name `tutorial`. With heap storage
enabled, the server uses that name to look up the prior heap snapshot and
save the new state after a successful run. The next call waits until the
previous one finishes, so it can read changes that have already been saved.
A different session name starts with its own state.

The five-second limit is for the V8 execution. The client also sets a
ten-second timeout on individual HTTP requests and stops starting new polls
after twenty seconds. A successful HTTP response can still describe a
failed JavaScript execution. The helper checks the terminal status and
reports the server’s error message.

```python
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
```

This example reads one page of console output, which is enough for the demo.
For longer output, use the pagination fields in the
[HTTP API reference](../reference/http-api.md).

## 4. Start the Sandbox and run the snippets

`App.lookup` gives the Sandbox a Modal app to belong to. The stock image’s
entrypoint starts `mcp-v8` on the configured port. These settings enable
directory-backed heap storage for the Sandbox lifetime.

| Setting | Purpose |
| --- | --- |
| `PORT=8080` | Starts the HTTP listener on the port used by the readiness probe and Connect Token. |
| `MCP_V8_HEAP_STORE=dir` | Enables V8 heap snapshots on disk. |
| `MCP_V8_HEAP_DIR=/data/heaps` | Stores the snapshot files. |
| `MCP_V8_SESSION_DB_PATH=/data/sessions` | Stores session records that associate a name with its snapshots. |

The TCP readiness probe checks that the listener is up before the client
mints its token. Modal blocks outbound network access from the Sandbox, and
the client uses the authenticated HTTP connection without opening a raw
tunnel.

The first two calls assign properties on `globalThis` so subsequent snippets
can read them. The third writes their sum to the console. The fourth
deliberately throws to exercise the failure path. The `finally` block
terminates the Sandbox even if a call fails, and the Sandbox’s three-minute
lifetime is a backstop. The directories under `/data` are local to this
Sandbox. Starting the example again creates a fresh session.

```python
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

Run the completed file.

```sh
python code_interpreter.py
```

After startup, the output should include the sum and the intentional error.

```text
15
Uncaught (in promise) Error: demo error
```

The two assignments produce no console output. To see what a fresh session
can read, change the third call to
`run_js(call, "console.log(x + y)", session="fresh")` and rerun the file.
The read raises a reference error because that session has no saved `x` or `y`.

## Embed V8 in Python with UniFFI

If your Python application needs to call the interpreter in-process,
download the `mcp_js-*.whl` asset from a
[GitHub release](https://github.com/r33drichards/mcp-js/releases) that includes
a wheel compatible with your Linux x86-64 system. From the download
directory, install that wheel into your virtual environment.

```sh
python -m pip install ./mcp_js-*.whl
```

The wheel bundles the native library and its generated UniFFI bindings.
`Engine` loads the library into the Python process, so this path needs no
HTTP server. Save the following as `embedded.py` and run
`python embedded.py`. It should print `15`.

```python
from mcp_js import Engine

with Engine(memory_mb=64, timeout_secs=5) as engine:
    result = engine.run_js("const x = 10; const y = 5; console.log(x + y)")
    if not result.ok:
        raise RuntimeError(result.error)
    print(result.output.rstrip())
```

The Python API is currently stateless across `run_js` calls, even on the
same `Engine` object. Put dependent statements in one call when embedding.
Use the Docker server’s named sessions when you want a notebook-like
sequence of separate calls. To retain Modal’s isolation with UniFFI, install
the wheel in the Python environment **inside** the Modal Sandbox and run
the Python interpreter there.
