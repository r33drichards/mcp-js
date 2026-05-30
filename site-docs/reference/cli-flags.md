# CLI Flags

`mcp-v8` is configured primarily through command-line flags. The major groups
are:

- storage: `--s3-bucket`, `--cache-dir`, `--directory-path`, `--stateless`
- transport: `--http-port`, `--sse-port`
- auth: `--jwks-url` (falls back to `JWKS_URL`)
- execution limits: `--heap-memory-max`, `--execution-timeout`,
  `--max-concurrent-executions`
- session logging: `--session-db-path`
- clustering: `--cluster-port`, `--node-id`, `--peers`, `--join`,
  `--advertise-addr`, `--heartbeat-interval`, `--election-timeout-min`,
  `--election-timeout-max`
- WASM: `--wasm-module`, `--wasm-config`, `--wasm-default-max-memory`
- fetch and policy: `--fetch-header`, `--fetch-header-config`,
  `--allow-external-modules`, `--policies-json`
- external MCP servers: `--mcp-server`, `--mcp-config`, `--mcp-stubs`,
  `--mcp-stub-prefix`
- utility: `--print-openapi`
