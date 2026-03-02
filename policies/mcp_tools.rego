package mcp.tools

default allow = false

# Allow specific server + tool combinations.
# Input schema:
#   {
#     "operation": "mcp_call_tool",
#     "server":    "server-name",
#     "tool":      "tool-name",
#     "arguments": { ... } or null
#   }

# ── Allowed server/tool pairs ────────────────────────────────────────────
# Add entries as {"server": "<name>", "tool": "<name>"} objects.
# Use "*" as tool name to allow all tools on a server.

allowed_tools := {
    # Example: allow all tools on a server named "math"
    # {"server": "math", "tool": "*"},
    # Example: allow a specific tool on a specific server
    # {"server": "db", "tool": "query"},
}

# Exact server + tool match
allow if {
    some entry in allowed_tools
    entry.server == input.server
    entry.tool == input.tool
}

# Wildcard: allow all tools on a server
allow if {
    some entry in allowed_tools
    entry.server == input.server
    entry.tool == "*"
}
