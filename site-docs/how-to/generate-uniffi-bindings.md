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

### Import the Python library

The public Python package exposes `from mcp_js import Engine`:

```python
from mcp_js import Engine

with Engine(memory_mb=64, timeout_secs=2) as engine:
    result = engine.run_js("console.log(6 * 7)")
    print(result.output)
```

This executes in-process, without shell commands or a running server. After
building the native library, prepare and install the package once:

```bash
python scripts/prepare-python-package.py \
  --library target/python-uniffi/release/libmcp_v8_uniffi.so
uv pip install ./python
```

The preparation command is build tooling, not part of your application.
Installed packages bundle the native library and generated private bindings.
See `python/README.md` in the repository for wheel packaging and result semantics.

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

### Builder-assembled engine configuration

`Engine.create(config)` is the general constructor. `EngineConfig` and its
nested records (`ExecutionLimits`, `FilesystemAccess`, `BlobStore`) derive a
UniFFI builder: each generated language gets a `<Record>Builder` object with
chainable setters and a `build()` that fails with
`RuntimeError::MissingRequiredField { record_type, field }` for the first
unset required field. Limits are required; hook-gated filesystem access, heap
persistence, and filesystem snapshots are independent optional axes, with
directory or S3 blob stores under an optional `data_dir`. `create_stateless`
and `create_with_filesystem` are conveniences over `create`.

### Host filesystem access from native callers

`Engine.create_with_filesystem(heap_mb, timeout_secs, filesystem_json)` builds
an engine whose only extra capability is hook/policy-gated host filesystem
access. `filesystem_json` is the `filesystem` entry of `--policies-json`
(`policies`, `pre`, `stack`), interpreted exactly as the server interprets it.
Guest code gets `fs.*`; the same engine also exports typed native methods
(`fs_read_file`, `fs_read_file_range`, `fs_read_text_file`, `fs_write_file`,
`fs_append_file`, `fs_stat`, `fs_lstat`, `fs_read_dir`, `fs_read_link`,
`fs_canonical_path`, `fs_make_dir`, `fs_remove`, `fs_rename`, `fs_exists`) that run through the same hook chain
and backend as the guest ops, so a pre hook rewrite or denial applies to both.
Bytes cross the boundary as `bytes`, never as JSON text, and failures are
`RuntimeError::FileSystem { kind, message }`. See `node/README.md` for the
Node.js shape of this API and `node/tests/filesystem.test.ts` for the
guest/native parity checks CI runs.

### Check all generated languages

Run the repository smoke check:

```bash
./scripts/check-uniffi-bindings.sh
```

The check regenerates Swift, Kotlin, Python, and Ruby bindings. It verifies
the canonical library, typed MCP request headers, and upstream MCP factory are
present in every generated surface.
