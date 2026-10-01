package mcp.subprocess

default allow = false

# Subprocess policy for gating Deno.Command and child_process.exec.
#
# Input schema:
#   {
#     "operation": "command_output" | "command_spawn" | "exec",
#     "command":   "/bin/sh" | "echo" | ...,
#     "args":      ["-c", "ls -la"] | ["hello"] | ...,
#     "cwd":       "/tmp" | null,
#     "env":       {"KEY": "VALUE"} | null
#   }

# ── Allowed commands for Deno.Command (command_output) ─────────────────
# Add command names to allow direct execution.

#
# Empty by default — written `set()` because `{}` is an empty *object*.
# Replace it with a set literal, e.g.
#
#   allowed_commands := {"echo", "cat", "ls"}
#
# The rules below use `in` and `strings.any_prefix_match` rather than
# iterating the sets: OPA (1.21+) rejects iteration over a set it can prove
# is empty, which would stop the server loading this file while the lists
# are still empty.

allowed_commands := set()

allow if {
    input.operation == "command_output"
    input.command in allowed_commands
}

# ── Allowed shell commands for child_process.exec ──────────────────────
# These patterns match the full command string passed to exec().
# For exec(), input.command is the shell (/bin/sh) and
# input.args[1] is the actual command string.

#
# Empty by default, e.g.
#
#   allowed_exec_patterns := {"echo", "ls"}

allowed_exec_patterns := set()

allow if {
    input.operation == "exec"
    strings.any_prefix_match(input.args[1], allowed_exec_patterns)
}

# ── Blanket allow for specific working directories ─────────────────────
# Uncomment to allow all subprocess operations within a specific directory.

# allow if {
#     input.cwd != null
#     startswith(input.cwd, "/tmp/sandbox/")
# }
