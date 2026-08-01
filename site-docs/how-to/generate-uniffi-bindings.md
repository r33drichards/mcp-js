# Generate native UniFFI bindings

Use this guide to generate language wrappers for the canonical `McpJsRuntime`
API from a local checkout.

## Prerequisites

- A Rust toolchain that can build the workspace.
- The repository's V8 build environment. `nix develop` is the supported setup
  when Nix is available.
- `uniffi-bindgen` version 0.32.0:

```bash
cargo install uniffi --version 0.32.0 --locked --features cli
```

## Generate Swift bindings

From the repository root, run:

```bash
./scripts/generate-uniffi-bindings.sh swift
```

The script builds `mcp-v8-uniffi` as a static library and writes generated
sources beneath `generated/uniffi/swift/`.

To choose a different output directory:

```bash
./scripts/generate-uniffi-bindings.sh swift /tmp/mcp-v8-swift
```

## Generate another language

The generator accepts the languages supported by UniFFI 0.32.0:

```bash
./scripts/generate-uniffi-bindings.sh kotlin
./scripts/generate-uniffi-bindings.sh python
./scripts/generate-uniffi-bindings.sh ruby
```

Generating wrappers does not by itself produce a loadable shared library for
Kotlin, Python, or Ruby. Check the [native bindings reference](../reference/uniffi-bindings.md)
before packaging those targets.

## Build a release artifact

Set `PROFILE=release`:

```bash
PROFILE=release ./scripts/generate-uniffi-bindings.sh swift
```

For a cross-compilation target, install the Rust target and set `TARGET`:

```bash
rustup target add aarch64-apple-ios
TARGET=aarch64-apple-ios PROFILE=release \
  ./scripts/generate-uniffi-bindings.sh swift generated/uniffi/ios-arm64
```

The target must also have a compatible V8 archive available through the build
environment.

## Verify the exported surface

Run the repository smoke check:

```bash
./scripts/check-uniffi-bindings.sh
```

The check regenerates Swift, Kotlin, Python, and Ruby bindings. It verifies
the canonical library, typed MCP request headers, and upstream MCP factory are
present in every generated surface.
