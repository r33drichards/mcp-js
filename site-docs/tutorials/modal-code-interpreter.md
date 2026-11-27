# Run a stateful JavaScript interpreter on Modal

Use the published `mcp-js` Docker image as the execution layer in
[Modal’s stateful interpreter example](https://modal.com/docs/examples/simple_code_interpreter).
The image runs the Rust `mcp-v8` server, which executes JavaScript in V8.
Your local Python script creates the Modal Sandbox and submits code over HTTP.
The Sandbox needs only the published image, with no Python installation or
custom Docker build.

## Run the example

Install the Modal SDK and authenticate, then run the example script from the
repository root.

```sh
python -m pip install modal
modal setup
git clone https://github.com/r33drichards/mcp-js.git
cd mcp-js
python examples/modal/code_interpreter.py
```

The script waits for the server on port 8080 and gets a
[Modal Sandbox Connect Token](https://modal.com/docs/guide/sandbox-networking#sandbox-connect-tokens)
for authenticated HTTP requests. It creates no public raw tunnel and blocks
outbound network access from the Sandbox.

Each call sends JavaScript to `POST /api/exec` with the same session name.
The server queues the execution and returns an ID. The client polls the
execution status and reads the captured console output. The first two calls
save `x` and `y` on the global object, the third prints `15`, and the fourth
reports a JavaScript error. See the [HTTP API reference](../reference/http-api.md)
for the request and result fields.

The server stores session data under `/data` inside the Sandbox. The example
terminates the Sandbox at exit, which also discards that data.

## Embed V8 in Python with UniFFI

For a Python application that needs an in-process interpreter, install the
Linux x86-64 wheel from a [GitHub release](https://github.com/r33drichards/mcp-js/releases)
that includes it. The PyPI package name is `mcp-js` once the project publishes it.

```sh
python -m pip install ./mcp_js-*.whl
```

```python
from mcp_js import Engine

with Engine(memory_mb=64, timeout_secs=5) as engine:
    result = engine.run_js("console.log(10 + 5)")
    if not result.ok:
        raise RuntimeError(result.error)
    print(result.output)
```

The current Python API uses a stateless engine. Keep the stateful sequence
above on the Docker server, or put all dependent statements into a single
`run_js` call when embedding. Run the Python application inside a Modal
Sandbox if it executes untrusted code and needs Modal’s isolation boundary.
