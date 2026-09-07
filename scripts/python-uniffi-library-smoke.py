"""Exercise JavaScript through the actual native UniFFI Python library."""
from __future__ import annotations

import json

import server as mcp_v8


def main() -> None:
    engine = mcp_v8.Engine.create_stateless(64, 2)

    def run(code: str) -> dict:
        return json.loads(engine.call_tool("run_js", json.dumps({"code": code}), None, None))

    try:
        assert "42" in run("console.log(6 * 7)")["output"]
        assert "awaited" in run('console.log(await Promise.resolve("awaited"))')["output"]
        assert "probe-error" in run('throw new Error("probe-error")')["error"]
        assert run("while (true) {} ").get("error"), "nonterminating JS must time out"
        assert "recovered" in run('console.log("recovered")')["output"]
        assert not engine.close().already_shutdown
        assert engine.close().already_shutdown
        try:
            run("console.log(1)")
        except mcp_v8.RuntimeError:
            pass
        else:
            raise AssertionError("execution after shutdown succeeded")
    finally:
        engine.close()

    for memory, timeout in ((0, 2), (64, 0), (4097, 2), (64, 301)):
        try:
            mcp_v8.Engine.create_stateless(memory, timeout)
        except mcp_v8.RuntimeError:
            pass
        else:
            raise AssertionError("invalid limits accepted")
    print("Python native JavaScript execution passed: output, await, errors, timeout recovery, shutdown")


if __name__ == "__main__":
    main()
