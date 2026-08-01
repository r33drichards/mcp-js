"""Load the generated UniFFI Python module and execute JavaScript through it."""

from __future__ import annotations

import asyncio
import json
import tempfile

import server as mcp_v8


def main() -> None:
    with tempfile.TemporaryDirectory(prefix="mcp-v8-python-uniffi-") as data_dir:
        config = mcp_v8.default_runtime_config(data_dir)
        library = mcp_v8.create_runtime(config)

        tool_names = {tool.name for tool in library.list_tools()}
        assert "run_js" in tool_names

        result = json.loads(
            library.call_tool(
                "run_js",
                json.dumps({"code": "console.log(6 * 7)"}),
                None,
                None,
            )
        )
        assert result["output"] == "42", result

        shutdown = asyncio.run(library.shutdown())
        assert shutdown.already_shutdown is False
        del library

    print("Python UniFFI library import and run_js smoke test passed")


if __name__ == "__main__":
    main()
