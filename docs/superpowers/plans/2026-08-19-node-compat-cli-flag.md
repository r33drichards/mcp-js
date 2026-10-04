# Node Compatibility CLI Flag Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Route Node core tests that spawn `process.execPath` into a fresh mcp-v8-backed child process through a hidden `--node-compat-cli` flag.

**Architecture:** `node_compat_full` remains the shard runner when invoked normally and becomes a Node-style child launcher when the hidden flag is present. The child parser accepts the startup flags required by `test-esm-import-flag.mjs`, builds an ordered ESM bootstrap over the existing virtual corpus module map, executes it with mcp-v8, and returns captured stdout/stderr and an exit code. The JS prelude only routes `process.execPath` to this child mode; arbitrary subprocesses continue using translated host paths.

**Tech Stack:** Rust 2024, deno_core/V8, sled console capture, shlex, JavaScript Node compatibility prelude.

## Global Constraints

- Linux only.
- Never run the complete corpus locally.
- No skips, assertion weakening, fixture-specific branches, or host Node fallback.
- Use `TMPDIR=/dev/shm` and `CARGO_TARGET_DIR=/dev/shm/mcp-js-node-compat-target`.
- Preserve the complete 16-shard GitHub Actions gate.

---

### Task 1: Parse hidden Node CLI invocations

**Files:**
- Modify: `server/examples/node_compat_full.rs`
- Modify: `server/Cargo.toml`

**Interfaces:**
- Produces: `NodeCliInvocation::parse(args, node_options) -> Result<NodeCliInvocation, String>`.
- Produces: ordered environment/CLI require/import lists, eval source, check mode, input type, and entrypoint.

- [ ] Write unit tests for repeated imports, `--flag=value`, `NODE_OPTIONS`, preload ordering, eval, check, and rejected unknown flags.
- [ ] Run `cargo test --manifest-path server/Cargo.toml --example node_compat_full node_cli` and verify the tests fail.
- [ ] Implement the explicit parser using `shlex::split` for `NODE_OPTIONS`.
- [ ] Rerun the focused example tests and require all parser tests to pass.

### Task 2: Execute the child with mcp-v8

**Files:**
- Modify: `server/examples/node_compat_full.rs`

**Interfaces:**
- Consumes: `NodeCliInvocation` and `CorpusModules`.
- Produces: `execute_node_cli(...) -> NodeCliResult { code, stdout, stderr }`.

- [ ] Add tests for virtualizing host corpus paths and generating Node startup order.
- [ ] Add a hidden-mode smoke test that prints `process.versions['mcp-v8']`.
- [ ] Reuse the existing module loader, subprocess policy, and console capture to execute a synthetic ESM bootstrap.
- [ ] Implement compile-only `--check` behavior by evaluating preloads but not the entrypoint body.
- [ ] Print captured output to real stdout, errors to stderr, and return the child exit code.

### Task 3: Route `process.execPath` to the child mode

**Files:**
- Modify: `server/tests/node_compat/runner/prelude.js`
- Modify: `server/examples/node_compat_full.rs`
- Test: `server/tests/node_builtins.rs`

**Interfaces:**
- Consumes: `std::env::current_exe()` as `__NODE_TEST_EXEC_PATH__`.
- Produces: `spawnPromisified(process.execPath, args)` invoking `[--node-compat-cli, ...args]` without translating virtual `/test` paths.

- [ ] Add a prelude source regression proving the private flag and virtual path behavior.
- [ ] Replace host `node` discovery with the current runner executable.
- [ ] Keep translation and output normalization for arbitrary host commands.
- [ ] Run `node_builtins` and example unit tests.

### Task 4: Verify the upstream frontier and push

**Files:**
- Modify: `server/src/engine/node_compat/process.js`
- Test: `server/tests/node_builtins.rs`

**Interfaces:**
- Produces: `process.config.variables.node_without_node_options === false` because the child honors `NODE_OPTIONS`.

- [ ] Run the exact unmodified `test/es-module/test-esm-import-flag.mjs` inventory with Node 26 corpus metadata.
- [ ] Run the prior two-case frontier inventory.
- [ ] Run `cargo check --locked` and `git diff --check`.
- [ ] Review the focused diff for critical or important findings.
- [ ] Commit, push, and monitor the strict 16-shard workflow.
