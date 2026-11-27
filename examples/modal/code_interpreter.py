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


def run_js(call, code):
    execution = call(
        "/api/exec",
        {"code": code, "session": "tutorial", "execution_timeout_secs": 5},
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
