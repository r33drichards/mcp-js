# Confines the example's file writes to the directory it renders into.
package mcp.filesystem

default allow = false

allow if {
    startswith(input.path, "/tmp/chart-out/")
}
