package mcp.fs

default allow = false

# ── Read operations ───────────────────────────────────────────────────
# Allow read-only operations on allowed paths.

allow if {
    input.operation == "readFile"
    path_allowed
    not path_denied
}

allow if {
    input.operation == "readdir"
    path_allowed
    not path_denied
}

allow if {
    input.operation == "stat"
    path_allowed
    not path_denied
}

allow if {
    input.operation == "exists"
    path_allowed
    not path_denied
}

# ── Write operations ──────────────────────────────────────────────────
# Allow write operations on allowed paths.

allow if {
    input.operation == "writeFile"
    path_allowed
    not path_denied
}

allow if {
    input.operation == "appendFile"
    path_allowed
    not path_denied
}

allow if {
    input.operation == "mkdir"
    path_allowed
    not path_denied
}

allow if {
    input.operation == "rm"
    path_allowed
    not path_denied
}

# ── Dual-path operations ─────────────────────────────────────────────
# Rename and copy require both source and destination to be allowed.

allow if {
    input.operation == "rename"
    path_allowed
    destination_allowed
    not path_denied
    not destination_denied
}

allow if {
    input.operation == "copyFile"
    path_allowed
    destination_allowed
    not path_denied
    not destination_denied
}

# ── Allowed path prefixes ────────────────────────────────────────────
# Paths under these prefixes are permitted. Add more as needed.

allowed_prefixes := {
    "/tmp/",
}

path_allowed if {
    some prefix in allowed_prefixes
    startswith(input.path, prefix)
}

# Also allow /tmp itself (for readdir, stat, etc.)
path_allowed if {
    input.path == "/tmp"
}

destination_allowed if {
    some prefix in allowed_prefixes
    startswith(input.destination, prefix)
}

# ── Denied path prefixes ─────────────────────────────────────────────
# These paths are always denied, even if they match an allowed prefix.
# Useful for blocking sensitive paths that might be under /tmp.

denied_prefixes := set()

path_denied if {
    some prefix in denied_prefixes
    startswith(input.path, prefix)
}

destination_denied if {
    some prefix in denied_prefixes
    startswith(input.destination, prefix)
}
