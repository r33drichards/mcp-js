# mcp-v8-uniffi proof of concept

This crate exposes the transport-agnostic MCP tool layer as a UniFFI library.
It deliberately keeps the FFI boundary small:

- `McpJsLibrary` owns Tokio plus the shared `server::runtime::McpJsRuntime`.
- `list_tools()` returns stable records with each tool's JSON Schema encoded as JSON.
- `call_tool()` accepts and returns JSON strings, matching `mcp_dispatch` without
  attempting to model arbitrary JSON in every target language.
- Stateless mode exposes blocking `run_js` semantics.
- Local stateful mode exposes the heap-backed tool set and persists data beneath
  a caller-provided directory.

## Why this boundary

UniFFI maps records, enums, strings, byte arrays, objects, and errors well, but
`serde_json::Value`, `rmcp::Tool`, Tokio runtime handles, and V8 types should stay
inside Rust. The shared `McpJsRuntime` wraps the existing `server::mcp_dispatch` seam, which
takes a tool name plus JSON arguments and returns JSON. The server CLI, HTTP API,
Streamable HTTP MCP, stdio MCP, legacy SSE MCP, and UniFFI wrapper now all hold
the same runtime facade.

## Generate bindings

The checked-in proof of concept builds as a `staticlib` on Linux because the
repository's prebuilt `rusty_v8` archive uses V8's executable-only TLS model.
UniFFI can extract proc-macro metadata from the static archive:

```sh
cargo build -p mcp-v8-uniffi --release
uniffi-bindgen generate \
  --library target/release/libmcp_v8_uniffi.a \
  --language swift \
  --out-dir generated/swift
```

A loadable `cdylib` for desktop Kotlin/Python/Ruby needs a custom V8 archive
built with `V8_FROM_SOURCE=1` and
`GN_ARGS=v8_monolithic_for_shared_library=true`, or an equivalent prebuilt
archive supplied through `RUSTY_V8_ARCHIVE`. Android V8 builds already select a
library-safe TLS model, but still require target-specific validation and AAR
packaging. iOS normally consumes the static library directly.

## Kotlin shape

```kotlin
val config = defaultLibraryConfig().copy(
    mode = LibraryMode.STATELESS,
)
val runtime = McpJsLibrary(config)
val tools = runtime.listTools()
val resultJson = runtime.callTool(
    name = "run_js",
    argumentsJson = """{"code":"console.log(1 + 1)"}""",
    sessionId = null,
    mcpHeadersJson = null,
)
```

For local stateful mode, set `mode` to `LOCAL_STATEFUL`, provide `dataDir`, call
`run_js`, and use the returned execution ID with `get_execution` and
`get_execution_output`.

## Next production steps

1. Move `McpJsRuntime`, the engine, and tool dispatch out of the broadly named
   `server` crate into a focused core crate so bindings do not inherit HTTP, CLI,
   and cluster dependencies.
2. Add a dedicated async job API for mobile callers rather than blocking a
   foreign thread for stateless `run_js`.
3. Decide which capabilities are safe to configure across FFI: fetch, external
   modules, subprocesses, filesystem mounts, upstream MCP servers, and WASM.
4. Add Android/iOS packaging, ABI matrix builds, and target-language smoke tests.
