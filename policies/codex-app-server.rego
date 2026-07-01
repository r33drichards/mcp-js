package mcp.subprocess

# OPA/Rego policy that lets mcp-v8's `run_js` runtime spawn the Codex
# `app-server` (and only that) as an interactive subprocess, so JavaScript can
# drive its stdio JSON-RPC protocol via Deno.Command(...).spawn().
#
# Enable with:
#   mcp-v8 --stateless \
#     --policies-json '{"subprocess":{"policies":[{"url":"file:///abs/path/to/policies/codex-app-server.rego"}]}}'
#
# Input schema (see engine/subprocess.rs):
#   {
#     "operation": "command_output" | "command_spawn" | "exec",
#     "command":   "codex" | "/abs/path/to/codex" | ...,
#     "args":      ["app-server"],
#     "cwd":       "/work" | null,
#     "env":       {"CODEX_HOME": "..."} | null
#   }

default allow = false

# Absolute paths and/or bare names of the Codex binary you allow to be spawned.
# Prefer an absolute path in production; a bare "codex" trusts PATH resolution.
allowed_codex_commands := {
    "codex",
    "codex-app-server",
}

# The interactive spawn used by Deno.Command(...).spawn().
allow if {
    input.operation == "command_spawn"
    allowed_codex_commands[_command_basename]
    input.args[0] == "app-server"
}

# Also permit an absolute path whose basename is an allowed command
# (e.g. "/usr/local/bin/codex" or the npm vendor path).
_command_basename := base if {
    parts := split(input.command, "/")
    base := parts[count(parts) - 1]
}
