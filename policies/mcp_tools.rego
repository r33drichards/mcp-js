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

#
# Empty by default — written `set()` because `{}` is an empty *object*.
# Replace it with a set literal, e.g.
#
#   allowed_tools := {
#       {"server": "math", "tool": "*"},     # all tools on server "math"
#       {"server": "db", "tool": "query"},   # one tool on one server
#   }
#
# The rules below test membership with `in` rather than iterating the set:
# OPA (1.21+) rejects iteration over a set it can prove is empty, which would
# stop the server loading this file while the list is still empty.

allowed_tools := set()

# Exact server + tool match
allow if {"server": input.server, "tool": input.tool} in allowed_tools

# Wildcard: allow all tools on a server
allow if {"server": input.server, "tool": "*"} in allowed_tools
