# Rust Client API

The Rust crate is `mcp-v8-client`, imported in code as `mcp_v8_client`. It is
generated from the server OpenAPI document and targets the HTTP API, not the
MCP transport.

Core entry points:

- `mcp_v8_client::Client` for the async HTTP client
- `mcp_v8_client::types::ExecRequest` for `POST /api/exec`

Useful first-pass methods on `Client` mirror the main execution endpoints:

- `exec_handler(&types::ExecRequest)` submits code for execution
- `list_executions_handler()` lists known executions
- `get_execution_handler(&id)` polls execution status and results
- `get_execution_output_handler(&id, line_offset, line_limit, byte_offset,
  byte_limit)` reads paginated console output
- `cancel_execution_handler(&id)` requests cancellation

The request type shown in the crate docs and README includes `code` plus
optional `heap`, `session`, `heap_memory_max_mb`, `execution_timeout_secs`,
and `tags` fields. The generated surface tracks the same HTTP endpoints
documented in `docs/http-api-and-client.md`.
