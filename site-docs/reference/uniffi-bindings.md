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
| Linux x64, `cdylib` | Used by the native JavaScript SDK CI | Build with the current pinned glibc V8 archive; validate ELF dependencies and host libc |
| iOS static library | Design supported, target validation pending | Requires an iOS-compatible V8 archive and Xcode packaging |
| Android shared library/AAR | Validation pending | Requires Android V8 builds, ABI matrix builds, and AAR packaging |
| Desktop Kotlin/Python/Ruby | Binding generation and Python smoke path available; distribution packaging is separate | Requires a compatible loadable library for the consumer host |

For the current Linux glibc build, use the Nix shell's pinned
`RUSTY_V8_ARCHIVE`. The old `V8_FROM_SOURCE=1` recipe is not a working shortcut
with the published V8 crate, which omits ICU build inputs. Follow the
[native JavaScript SDK build guide](../how-to/native-javascript-sdk.md#build-and-validate-a-package)
and package checks. A successful static archive build alone does not establish
shared-library portability on another host.

The Node.js SDK uses the loadable `mcp-v8-uniffi-python` library with generated
Node-API wrappers. Its Linux x64 glibc/musl release matrix is documented
[separately](../how-to/native-javascript-sdk.md), not implied by the Swift/Kotlin
binding generator's output.

## Generation command

Use the repository script rather than invoking `uniffi-bindgen` directly:

```bash
./scripts/generate-uniffi-bindings.sh swift
```

See [Generate native UniFFI bindings](../how-to/generate-uniffi-bindings.md) for
the complete procedure and cross-compilation options.
