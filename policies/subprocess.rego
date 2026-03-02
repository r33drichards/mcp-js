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

allowed_commands := {
    # Example: allow echo and cat
    # "echo",
    # "cat",
    # "ls",
}

allow if {
    input.operation == "command_output"
    allowed_commands[input.command]
}

# ── Allowed shell commands for child_process.exec ──────────────────────
# These patterns match the full command string passed to exec().
# For exec(), input.command is the shell (/bin/sh) and
# input.args[1] is the actual command string.

allowed_exec_patterns := {
    # Example: allow "echo" commands
    # "echo",
    # "ls",
}

allow if {
    input.operation == "exec"
    some pattern in allowed_exec_patterns
    startswith(input.args[1], pattern)
}

# ── Blanket allow for specific working directories ─────────────────────
# Uncomment to allow all subprocess operations within a specific directory.

# allow if {
#     input.cwd != null
#     startswith(input.cwd, "/tmp/sandbox/")
# }
