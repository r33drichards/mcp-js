package mcp.filesystem

# Permissive filesystem policy - allows all operations.
# Intended for use in sandboxed environments (Docker containers, etc.)
# where the mcp-js server should have full filesystem access.
#
# Usage:
#   --policies-json '{"filesystem":{"policies":[{"url":"file:///path/to/filesystem_allow_all.rego"}]}}'

default allow = true
