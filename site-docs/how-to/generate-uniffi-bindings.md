# Generate native UniFFI bindings

Use this guide to generate language wrappers for the canonical `Engine`
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

### Run the Python smoke test locally

With `uv` installed, enter `nix develop`, install `uniffi-bindgen` as shown
above, then run:

```bash
uv run scripts/test-python-uniffi.py
```

This builds the Python-loadable shared library, generates fresh Python
bindings in a temporary directory, and imports them using uv's Python.
It does not start or connect to an HTTP server. The first run builds V8 from
source with shared-library-compatible flags and can take a long time; subsequent
runs reuse `target/python-uniffi`. Linux and macOS are supported.

To test a shared library you already built:

```bash
uv run scripts/test-python-uniffi.py \
  --library target/python-uniffi/release/libmcp_v8_uniffi.so
```

On macOS use `libmcp_v8_uniffi.dylib`. The test creates a native stateless
engine and checks JavaScript output, Promise awaiting, thrown errors, timeout
recovery, and idempotent shutdown.

With generated bindings on `PYTHONPATH`, synchronous embedding looks like:

```python
import json
import server

engine = server.Engine.create_stateless(64, 2)  # heap MB, default timeout seconds
try:
    result = json.loads(engine.call_tool(
        "run_js", json.dumps({"code": "console.log(6 * 7)"}), None, None,
    ))
    assert "42" in result["output"]
finally:
    engine.close()
```

Factory limits are 16-4096 MB and 1-300 seconds. Each engine permits one V8
execution at a time. Network, filesystem, subprocess, and external module
capabilities are disabled by default. Its execution database is temporary.
The synchronous methods own a Tokio runtime and do not require Python asyncio.
Close the engine explicitly; dropping the last reference releases its runtime
without blocking a Tokio worker. Per-call execution options retain the existing
`run_js` semantics; factory limits are defaults, not a security boundary against
the embedding Python application.

### Check all generated languages

Run the repository smoke check:

```bash
./scripts/check-uniffi-bindings.sh
```

The check regenerates Swift, Kotlin, Python, and Ruby bindings. It verifies
the canonical library, typed MCP request headers, and upstream MCP factory are
present in every generated surface.
