package mcp.subprocess

default allow = false















allowed_commands := {
    
    
    
    
}

allow if {
    input.operation == "command_output"
    allowed_commands[input.command]
}






allowed_exec_patterns := {
    
    
    
}

allow if {
    input.operation == "exec"
    some pattern in allowed_exec_patterns
    startswith(input.args[1], pattern)
}








