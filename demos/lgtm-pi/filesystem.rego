package mcp.filesystem
default allow = false
allow if {
 input.operation in {"readFile", "writeFile", "stat", "exists"}
 regex.match("^/dashboards/[a-zA-Z0-9_-]+[.]json$", input.path)
 not input.destination
}
