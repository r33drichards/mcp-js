# mcp-v8 UniFFI library

This crate packages the canonical `server::library::McpJsLibrary` API as a Rust
`staticlib` with UniFFI metadata. The CLI, HTTP API, stdio and Streamable HTTP
MCP servers, and legacy SSE server use the same library facade.

The foreign-language boundary uses typed records for fixed configuration and
lifecycle data. Arbitrary JavaScript code, tool arguments, tool results, JSON
Schema, and session snapshots remain JSON strings.

## Generate bindings

```bash
cargo install uniffi --version 0.32.0 --locked --features cli
./scripts/generate-uniffi-bindings.sh swift
./scripts/check-uniffi-bindings.sh
```

See the documentation site for:

- [the binding generation how-to](../site-docs/how-to/generate-uniffi-bindings.md)
- [the native binding and platform reference](../site-docs/reference/uniffi-bindings.md)

The crate currently builds `staticlib` and `rlib` artifacts. A Linux `cdylib`
using the repository's prebuilt V8 archive is not supported because that archive
uses V8's executable-only TLS model.
