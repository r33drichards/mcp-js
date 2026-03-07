package mcp.filesystem

default allow = false

# All filesystem operations are restricted to /data/workspace/ subtree.
# Path traversal (../) is explicitly blocked.

allow if {
    not path_traversal
    path_allowed
}

# Block any path containing ".." components
path_traversal if {
    contains(input.path, "..")
}

path_traversal if {
    input.destination
    contains(input.destination, "..")
}

# Primary path must start with /data/workspace/
path_allowed if {
    startswith(input.path, "/data/workspace/")
    check_destination
}

# For rename/copyFile, destination must also be under /data/workspace/
check_destination if {
    not input.destination
}

check_destination if {
    input.destination
    startswith(input.destination, "/data/workspace/")
}
