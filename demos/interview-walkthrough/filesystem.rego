package mcp.filesystem

default allow = false

# Synthetic demo namespace. Exact operations and segment-aware paths.
# Reject traversal rather than relying on lexical startswith alone.
safe_path(p) if {
    allowed_prefix(p)
    not contains(p, "/../")
    not endswith(p, "/..")
    not contains(p, "/./")
    not contains(p, "//")
}
allow if {
    input.operation in {"readFile", "writeFile", "readdir", "stat", "mkdir", "exists"}
    safe_path(input.path)
    not input.destination
}

allowed_prefix(p) if { startswith(p, "/work/allowed/") }
allowed_prefix(p) if { startswith(p, "/work/scratch/") }
allowed_prefix(p) if { startswith(p, "/work/dashboards/") }
