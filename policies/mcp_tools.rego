package mcp.tools

default allow = false














allowed_tools := {
    
    
    
    
}


allow if {
    some entry in allowed_tools
    entry.server == input.server
    entry.tool == input.tool
}


allow if {
    some entry in allowed_tools
    entry.server == input.server
    entry.tool == "*"
}
