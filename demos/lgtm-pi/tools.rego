package mcp.tools
default allow = false
allow if {
 input.server == "grafana"
 input.tool in {"query_prometheus", "query_loki_logs", "list_datasources", "get_datasource_by_uid"}
}
allow if { input.server == "browser"; input.tool == "browser_execute" }
