"""Load the generated UniFFI Python module and check the exported surface.

Construction now happens host-side in Rust (bootstrap); the bindings expose
the Engine object plus its records, so this smoke test verifies the module
imports and the expected symbols exist.
"""

from __future__ import annotations

import server as mcp_v8


def main() -> None:
    for symbol in (
        "Engine",
        "ExecutionRequest",
        "ToolCallRequest",
        "McpRequestHeaders",
        "RuntimeError",
        "SessionSnapshotView",
        "FsPushOutcome",
        "ExecutionInfo",
    ):
        assert hasattr(mcp_v8, symbol), f"missing exported symbol: {symbol}"

    print("Python UniFFI module import smoke test passed")


if __name__ == "__main__":
    main()
