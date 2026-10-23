package mcp.filesystem
default allow = false
allow if {
 input.operation in {"readFile", "writeFile", "stat", "exists", "readdir", "mkdir"}
 startswith(input.path,"/work/")
 not contains(input.path,"..")
 not input.destination
}
