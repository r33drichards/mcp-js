package mcp.tools
default allow = false
allow if {
 input.server == "grafana"
 input.tool in {"query_prometheus", "query_loki_logs", "list_datasources", "get_datasource_by_uid"}
}
allow if { input.server == "browser"; input.tool == "browser_execute" }

# Explicit COW snapshots only; no implicit shared upstream session or host file execution.
allow if {
 input.server == "scratch"
 input.tool == "run_js"
 is_string(input.arguments.code)
 is_string(input.arguments.fs)
 not input.arguments.file
}
allow if {
 input.server == "scratch"
 input.tool in {"get_execution", "get_execution_output"}
}
