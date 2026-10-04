# Goal: Minimal embedded JavaScript execution from Python

Status: Implemented and native execution validated on Linux x86_64 (2026-09-06). Not published.

## Outcome

A Python program can create a stateless mcp-js engine, execute JavaScript in
process through UniFFI, inspect output/errors, and shut down cleanly. No HTTP
server, MCP server process, or separately running Rust host is required.

## Smallest useful scope

1. Export a Python-callable stateless Engine factory with safe defaults and
   validated memory/execution timeout limits. Reuse the canonical Rust bootstrap
   and execution path rather than creating a second engine implementation.
2. Initialize V8 safely, create an execution registry, and retain a Tokio runtime
   for the engine lifetime. Keep temporary storage alive until shutdown/drop.
3. Support synchronous Python execution through the existing call_tool/run_js
   path backed by the owned runtime. Provide a Python-callable synchronous close
   path with idempotent shutdown and safe runtime destruction. Do not require
   asyncio integration for this initial implementation.
4. Upgrade scripts/test-python-uniffi.py and the native Python smoke test to
   execute real JavaScript using the actual generated bindings and shared
   library, not mocked bindings or an HTTP endpoint.
5. Document the exact uv command and native build prerequisites, and make CI run
   the same real execution test.

## Acceptance criteria

- `uv run scripts/test-python-uniffi.py` builds/generates/loads the native Python
  bindings and runs execution assertions successfully on a supported host.
- An existing shared library can be tested with `--library` without rebuilding.
- Python creates a stateless engine without a Rust host or server process.
- `console.log(6 * 7)` yields captured output containing `42`.
- JavaScript awaiting a Promise completes with the expected output.
- A thrown JavaScript error is reported distinctly from successful execution.
- A nonterminating script is stopped by the configured execution deadline, and
  a subsequent normal execution succeeds.
- Repeated calls work on one engine; shutdown is idempotent, post-shutdown calls
  fail clearly, and process exit does not hang or panic.
- Invalid limits fail with useful errors rather than panics.
- Existing CLI/MCP behavior and focused Rust tests remain passing.
- Report actual native execution proof, platform, and command; generated-symbol
  checks alone do not satisfy this goal.

## Non-goals

- Stateful sessions, heap persistence, filesystem mounts, and cluster support.
- Broad policy/configuration builders or enabling network/filesystem access by
  default.
- Full Python asyncio integration or concurrent cancellation API support.
- Publishing wheels or merging PR #224 without separate authorization.

## Implementation notes

- Factory/runtime gap: server/src/engine/ffi.rs and server/src/bootstrap.rs.
- Python packaging: mcp-v8-uniffi-python/.
- Existing synchronous call_tool requires a library-owned Tokio runtime.
- Existing exported async shutdown does not by itself provide synchronous
  Python lifecycle support; implement and test an appropriate close bridge.
- Shared-library-compatible V8 compilation and a real Python import are required
  validation gates, not assumed consequences of static binding generation.
- Preserve existing workspace edits; inspect the current merge/publication state
  before implementing or committing.

## Validation evidence

- `uv run scripts/test-python-uniffi.py` passed against the source-built shared
  library using local Clang 20 and `EXTRA_GN_ARGS=use_glib=false`.
- `uv run scripts/test-python-uniffi.py --library
  target/python-uniffi/release/libmcp_v8_uniffi.so` passed independently without
  the source-build environment overrides.
- Both native Python runs verified output, Promise awaiting, JavaScript errors,
  deadline termination, recovery, repeated shutdown, rejection after shutdown,
  and invalid limits.
- Three embedded Rust regression tests and five Python runner tests passed.
- Local logs: `.local-validation/python-execution-final.log`,
  `.local-validation/python-existing-library.log`, and
  `.local-validation/rust-execution.log`.
