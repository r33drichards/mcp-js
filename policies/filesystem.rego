package mcp.filesystem

default allow = false




allow if {
    input.claims
    input.claims.sub
    input.claims.session_id
    workspace_prefix := concat("", ["/data/workspace/", input.claims.sub, "/", input.claims.session_id, "/"])
    startswith(input.path, workspace_prefix)
    check_destination
}

check_destination if {
    not input.destination
}

check_destination if {
    input.destination
    input.claims
    input.claims.sub
    input.claims.session_id
    workspace_prefix := concat("", ["/data/workspace/", input.claims.sub, "/", input.claims.session_id, "/"])
    startswith(input.destination, workspace_prefix)
}
