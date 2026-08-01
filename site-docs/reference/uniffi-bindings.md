# Native UniFFI bindings

This page records the native binding artifacts, exported API groups, and known
platform constraints for `mcp-v8`.

## Source crate

| Item | Value |
|---|---|
| Package | `mcp-v8-uniffi` |
| Rust library name | `mcp_v8_uniffi` |
| Crate types | `staticlib`, `rlib` |
| UniFFI version | 0.32.0 |
| Swift module | `McpV8` |
| Swift FFI module | `McpV8FFI` |
| Kotlin package | `dev.mcpv8` |

The UniFFI metadata is defined by proc-macro exports in the `server` crate and
linked into the `mcp-v8-uniffi` static archive.

## Generated outputs

| Language | Typical generated files | Native library requirement |
|---|---|---|
| Swift | `server.swift`, C header, module map | Can link the static archive directly |
| Kotlin | Kotlin wrapper plus JNA/JNI support files | Requires a platform-loadable native library |
| Python | Python wrapper | Requires a platform-loadable native library |
| Ruby | Ruby wrapper | Requires a platform-loadable native library |

## Exported API groups

The generated bindings expose these stable groups:

- Runtime, storage, hardening, WASM, prompt, policy, fetch-auth, capability, and
  upstream MCP configuration records.
- `Engine` construction, capabilities, lifecycle state, and shutdown.
- Tool discovery plus synchronous and asynchronous tool invocation.
- Asynchronous execution submission, status, output pagination, cancellation,
  and listing.
- Session history, heap tags, filesystem labels, push, reset, and merge.
- Typed MCP request headers through `McpRequestHeaders`.

Arbitrary JavaScript code, tool arguments, tool results, schemas, and session
snapshots remain JSON strings because their shapes are not fixed across tools.

## Platform status

| Target | Status | Constraint |
|---|---|---|
| Linux host, static archive | Verified | Builds and generates Swift, Kotlin, Python, and Ruby bindings in CI |
| Linux host, `cdylib` | Blocked with the repository's prebuilt V8 | V8 uses the executable-only TLS model |
| iOS static library | Design supported, target validation pending | Requires an iOS-compatible V8 archive and Xcode packaging |
| Android shared library/AAR | Validation pending | Requires Android V8 builds, ABI matrix builds, and AAR packaging |
| Desktop Kotlin/Python/Ruby | Packaging pending | Requires a loadable shared library built with shared-library-safe V8 TLS |

A Linux `cdylib` requires a V8 archive built for shared-library use, for example
with `V8_FROM_SOURCE=1` and
`GN_ARGS=v8_monolithic_for_shared_library=true`, or an equivalent archive
provided through `RUSTY_V8_ARCHIVE`.

## Generation command

Use the repository script rather than invoking `uniffi-bindgen` directly:

```bash
./scripts/generate-uniffi-bindings.sh swift
```

See [Generate native UniFFI bindings](../how-to/generate-uniffi-bindings.md) for
the complete procedure and cross-compilation options.
