package mcp.filesystem

default allow = false

# Filesystem operations are restricted to /data/workspace/<session_id>/.

allow if {
    input.session_id
    session_prefix := concat("", ["/data/workspace/", input.session_id, "/"])
    startswith(input.path, session_prefix)
    check_destination
}

check_destination if {
    not input.destination
}

check_destination if {
    input.destination
    input.session_id
    session_prefix := concat("", ["/data/workspace/", input.session_id, "/"])
    startswith(input.destination, session_prefix)
}
