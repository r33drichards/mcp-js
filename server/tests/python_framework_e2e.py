"""FastAPI -> mcp-js -> FastMCP end-to-end integration test."""

from __future__ import annotations

import asyncio
import json
import os
import socket
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import Any

import httpx
from fastapi import FastAPI, HTTPException
from fastmcp import FastMCP
from pydantic import BaseModel

SCRIPT_PATH = Path(__file__).resolve()


def available_port() -> int:
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        return listener.getsockname()[1]


def fastmcp_server() -> None:
    mcp = FastMCP("python-framework-e2e")

    @mcp.tool
    def framework_probe(message: str) -> dict[str, str]:
        """Return a sentinel proving the Python FastMCP tool executed."""
        return {"framework": "fastmcp", "message": message}

    mcp.run(transport="stdio", show_banner=False)


class ProbeRequest(BaseModel):
    message: str


def create_gateway() -> FastAPI:
    app = FastAPI(title="mcp-js Python framework E2E gateway")
    mcp_js_url = os.environ["MCP_JS_URL"].rstrip("/")

    @app.get("/health")
    async def health() -> dict[str, str]:
        return {"status": "ok", "framework": "fastapi"}

    @app.post("/probe")
    async def probe(request: ProbeRequest) -> dict[str, Any]:
        message = json.dumps(request.message)
        code = (
            'const result = await mcp.callTool("python", "framework_probe", '
            f"{{ message: {message} }}); console.log(JSON.stringify(result));"
        )

        async with httpx.AsyncClient(timeout=10.0) as client:
            submit = await client.post(f"{mcp_js_url}/api/exec", json={"code": code})
            if submit.status_code != 202:
                raise HTTPException(submit.status_code, submit.text)

            execution_id = submit.json()["execution_id"]
            status: dict[str, Any] = {}
            for _ in range(100):
                response = await client.get(
                    f"{mcp_js_url}/api/executions/{execution_id}"
                )
                response.raise_for_status()
                status = response.json()
                if status["status"] == "completed":
                    break
                if status["status"] in {"failed", "cancelled", "timed_out"}:
                    raise HTTPException(502, status)
                await asyncio.sleep(0.1)
            else:
                raise HTTPException(504, "mcp-js execution did not complete")

            output = await client.get(
                f"{mcp_js_url}/api/executions/{execution_id}/output"
            )
            output.raise_for_status()
            return {
                "gateway": "fastapi",
                "execution_id": execution_id,
                "output": output.json()["data"],
            }

    return app


def run_gateway() -> None:
    import uvicorn

    uvicorn.run(
        create_gateway(),
        host="127.0.0.1",
        port=int(os.environ["FASTAPI_PORT"]),
        log_level="warning",
    )


def wait_for_url(url: str, process: subprocess.Popen[bytes], timeout: float) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError(f"process exited before {url} became ready")
        try:
            response = httpx.get(url, timeout=1.0)
            if response.is_success:
                return
        except httpx.HTTPError:
            pass
        time.sleep(0.1)
    raise RuntimeError(f"timed out waiting for {url}")


def stop_process(process: subprocess.Popen[bytes]) -> None:
    if process.poll() is not None:
        return
    process.terminate()
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=5)


def run_test(server_binary: str) -> None:
    mcp_port = available_port()
    fastapi_port = available_port()

    with tempfile.TemporaryDirectory(prefix="mcp-js-python-e2e-") as temp_dir:
        temp_path = Path(temp_dir)
        mcp_log_path = temp_path / "mcp-js.log"
        gateway_log_path = temp_path / "fastapi.log"

        upstream = f"python=stdio:{sys.executable}:{SCRIPT_PATH}:fastmcp"
        with (
            mcp_log_path.open("wb") as mcp_log,
            gateway_log_path.open("wb") as gateway_log,
        ):
            mcp_process = subprocess.Popen(
                [
                    server_binary,
                    "--http-port",
                    str(mcp_port),
                    "--mcp-server",
                    upstream,
                ],
                stdout=mcp_log,
                stderr=subprocess.STDOUT,
            )
            gateway_process: subprocess.Popen[bytes] | None = None
            try:
                wait_for_url(
                    f"http://127.0.0.1:{mcp_port}/api/version",
                    mcp_process,
                    30.0,
                )

                gateway_env = os.environ.copy()
                gateway_env.update(
                    {
                        "MCP_JS_URL": f"http://127.0.0.1:{mcp_port}",
                        "FASTAPI_PORT": str(fastapi_port),
                    }
                )
                gateway_process = subprocess.Popen(
                    [sys.executable, str(SCRIPT_PATH), "gateway"],
                    env=gateway_env,
                    stdout=gateway_log,
                    stderr=subprocess.STDOUT,
                )
                wait_for_url(
                    f"http://127.0.0.1:{fastapi_port}/health",
                    gateway_process,
                    30.0,
                )

                response = httpx.post(
                    f"http://127.0.0.1:{fastapi_port}/probe",
                    json={"message": "integration-ok"},
                    timeout=30.0,
                )
                response.raise_for_status()
                result = response.json()
                assert result["gateway"] == "fastapi"
                assert "fastmcp" in result["output"]
                assert "integration-ok" in result["output"]
            except Exception:
                mcp_log.flush()
                gateway_log.flush()
                print("--- mcp-js log ---", file=sys.stderr)
                print(mcp_log_path.read_text(errors="replace"), file=sys.stderr)
                print("--- FastAPI log ---", file=sys.stderr)
                print(gateway_log_path.read_text(errors="replace"), file=sys.stderr)
                raise
            finally:
                if gateway_process is not None:
                    stop_process(gateway_process)
                stop_process(mcp_process)

    print("FastAPI -> mcp-js -> FastMCP E2E test passed")


def main() -> None:
    if len(sys.argv) == 2 and sys.argv[1] == "fastmcp":
        fastmcp_server()
        return
    if len(sys.argv) == 2 and sys.argv[1] == "gateway":
        run_gateway()
        return
    if len(sys.argv) == 2:
        run_test(sys.argv[1])
        return
    raise SystemExit(f"usage: {sys.argv[0]} <server-binary|fastmcp|gateway>")


if __name__ == "__main__":
    main()
