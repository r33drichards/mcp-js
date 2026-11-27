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
