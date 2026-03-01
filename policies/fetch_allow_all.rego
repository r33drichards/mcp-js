package mcp.fetch

# Permissive fetch policy - allows all HTTP methods and all domains.
# Intended for use in sandboxed environments (Docker containers, etc.)
# where the mcp-js server should have unrestricted network access.
#
# Usage (combined with filesystem):
#   --policies-json '{"fetch":{"policies":[{"url":"file:///path/to/fetch_allow_all.rego"}]},"filesystem":{"policies":[{"url":"file:///path/to/filesystem_allow_all.rego"}]}}'

default allow = true
