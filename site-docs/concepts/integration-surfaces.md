# Integration Surfaces

`mcp-v8` can be used through several integration surfaces:

- MCP clients such as Claude Desktop, Claude Code, or Cursor
- the plain HTTP API
- the `mcp-v8-cli`
- the `mcp-v8-client` Rust crate

```mermaid
flowchart TD
  A[MCP client] --> B[MCP tools]
  C[CLI] --> D[HTTP API]
  E[Rust client] --> D
  D --> F[execution engine]
  B --> F
```

These surfaces share the same underlying execution engine, but they present it
in different forms:

- MCP exposes the runtime as tools
- the HTTP API exposes request and response endpoints directly
- the CLI wraps the HTTP API for shell use
- the Rust client provides typed access to the same HTTP endpoints

`mcp-v8` can also connect to upstream MCP servers and expose their tools
inside the JavaScript runtime, with optional stub tools on its own MCP surface
for discovery. See [MCP Pass-Through](mcp-pass-through.md).
