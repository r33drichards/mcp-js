# Rust Client API

The generated Rust client targets the HTTP API, not the MCP transport.

At a minimum, the reference should cover:

- `mcp_v8_client::Client`
- the generated request and response types for `/api/exec`
- execution status polling
- console output pagination
- cancellation calls

This page should stay focused on the typed client surface and how it maps to
the HTTP API.
