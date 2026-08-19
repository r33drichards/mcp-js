# Node Globals And Modal Tutorial Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an opt-in `--node-globals` runtime bootstrap for `Buffer` and synthetic `process`, then update the Modal tutorial to use it with the bundled SDK import.

**Architecture:** Thread a boolean from clap/config through `Engine` and `ExecutionConfig`. Before loading the user entry module, evaluate a small internal ESM bootstrap that imports the existing `node:buffer` and `node:process` compatibility modules and assigns them to `globalThis`. Keep the default disabled and regenerate the CLI/config/Nix references from the existing metadata-driven tooling.

**Tech Stack:** Rust, clap, deno_core/V8, Tokio integration tests, MkDocs Markdown, Nix documentation generators.

## Global Constraints

- `--node-globals` remains disabled by default.
- Only `globalThis.Buffer` and `globalThis.process` are installed.
- Reuse the existing embedded Node compatibility modules.
- Do not expose host environment or process state.
- Do not add `node:child_process` or broaden host capabilities.
- The Modal example must use `npm:modal?target=node&bundle`.

---

### Task 1: CLI And Configuration Surface

**Files:**
- Modify: `server/src/cli.rs`
- Modify: `server/src/config.rs`
- Regenerate: `site-docs/reference/cli-flags.md`
- Regenerate: `site-docs/reference/config-file.md`
- Regenerate: `nix/options.nix`

**Interfaces:**
- Produces: `Cli::node_globals: bool`
- Consumes: existing clap/config precedence machinery

- [ ] **Step 1: Add failing CLI and config tests**

Add assertions that `--node-globals`, `MCP_V8_NODE_GLOBALS=true`, and `node_globals = true` produce `Cli::node_globals == true`, while the default remains false.

- [ ] **Step 2: Run focused tests and verify failure**

Run:

```bash
cargo test -p server cli::tests config::tests
```

Expected: compilation or assertion failure because `Cli::node_globals` does not exist.

- [ ] **Step 3: Add the clap field**

Add a `Module Import` boolean option with:

```rust
#[arg(
    long = "node-globals",
    env = "MCP_V8_NODE_GLOBALS",
    default_value = "false",
    help_heading = "Module Import"
)]
pub node_globals: bool,
```

Document that it installs the sandboxed `Buffer` and `process` values before user modules evaluate.

- [ ] **Step 4: Run focused tests and verify success**

Run:

```bash
cargo test -p server cli::tests config::tests
```

Expected: all focused tests pass.

- [ ] **Step 5: Commit the configuration surface**

```bash
git add server/src/cli.rs server/src/config.rs
git commit -m "feat: add node globals option"
```

### Task 2: Pre-Evaluation Runtime Bootstrap

**Files:**
- Modify: `server/src/engine/node_compat.rs`
- Modify: `server/src/engine/mod.rs`
- Modify: `server/src/main.rs`
- Create: `server/tests/node_globals.rs`

**Interfaces:**
- Consumes: `Cli::node_globals`
- Produces: `Engine::with_node_globals(bool)` and `ExecutionConfig::node_globals(bool)`

- [ ] **Step 1: Add failing runtime tests**

Create tests proving:

```javascript
console.log(typeof Buffer, typeof process);
```

reports both values as undefined by default and as `function object` when enabled. Add a local HTTP-served static dependency whose top-level code throws unless both globals already exist; statically import it from the user entry module.

- [ ] **Step 2: Run runtime tests and verify failure**

Run:

```bash
cargo test -p server --test node_globals -- --nocapture
```

Expected: compilation failure because `with_node_globals` is absent, or enabled-behavior assertions fail.

- [ ] **Step 3: Implement the bootstrap module**

Add an internal module source equivalent to:

```javascript
import { Buffer } from 'node:buffer';
import process from 'node:process';
globalThis.Buffer = Buffer;
globalThis.process = process;
```

Evaluate it to completion inside `execute_module` before loading the user entry module when `ExecutionConfig::node_globals` is true.

- [ ] **Step 4: Thread the option through Engine**

Add the boolean to `Engine`, constructors, builder methods, stateless execution, and stateful execution. Pass `cli.node_globals` from `server/src/main.rs` and log whether the bootstrap is enabled.

- [ ] **Step 5: Run runtime and Node compatibility tests**

Run:

```bash
cargo test -p server --test node_globals -- --nocapture
cargo test -p server --test node_builtins --test node_compat -- --nocapture
```

Expected: all tests pass and the existing compatibility baseline does not drift.

- [ ] **Step 6: Commit the runtime behavior**

```bash
git add server/src/engine/node_compat.rs server/src/engine/mod.rs server/src/main.rs server/tests/node_globals.rs
git commit -m "feat: bootstrap optional Node globals"
```

### Task 3: Modal Tutorial And Generated References

**Files:**
- Modify: `site-docs/tutorials/modal.md`
- Regenerate: `site-docs/reference/cli-flags.md`
- Regenerate: `site-docs/reference/config-file.md`
- Regenerate: `nix/options.nix`

**Interfaces:**
- Consumes: `--node-globals`, `MCP_V8_NODE_GLOBALS`
- Produces: current Modal setup and examples

- [ ] **Step 1: Update the tutorial**

Add `--node-globals` to the server command and `MCP_V8_NODE_GLOBALS=true` to container and Railway configuration. Replace the manual global assignments with:

```javascript
import { ModalClient } from 'npm:modal?target=node&bundle';
```

Explain that the flag installs sandboxed globals before dependency evaluation and that `bundle` avoids the optional native-detection dependency path.

- [ ] **Step 2: Regenerate reference documentation**

Run the repository's Nix-backed documentation generation commands:

```bash
nix develop -c bash -lc '
  cargo build -p server --bin generate-cli-markdown --bin generate-config-markdown --bin generate-nix-options &&
  ./target/debug/generate-cli-markdown > site-docs/reference/cli-flags.md &&
  ./target/debug/generate-config-markdown > site-docs/reference/config-file.md &&
  ./target/debug/generate-nix-options > nix/options.nix
'
```

Expected: generated references include the new CLI, environment, config, and Nix option surfaces.

- [ ] **Step 3: Run documentation drift checks**

Run:

```bash
nix flake check -L
```

If the complete flake check is impractical, run the focused docs check exposed by the flake and record the limitation.

- [ ] **Step 4: Commit documentation**

```bash
git add site-docs/tutorials/modal.md site-docs/reference/cli-flags.md site-docs/reference/config-file.md nix/options.nix
git commit -m "docs: update Modal for Node globals"
```

### Task 4: Final Verification And Pull Request

**Files:**
- Verify all changed files

**Interfaces:**
- Consumes: completed implementation and documentation
- Produces: pushed branch and GitHub pull request

- [ ] **Step 1: Run formatting and focused tests**

Run:

```bash
cargo fmt --all -- --check
cargo test -p server --test node_globals --test node_builtins --test node_compat -- --nocapture
cargo test -p server cli::tests config::tests
```

Expected: formatting and all focused tests pass.

- [ ] **Step 2: Run broader validation**

Run:

```bash
cargo test -p server
```

Expected: all non-environmental tests pass. Report unrelated or infrastructure-only failures without modifying unrelated code.

- [ ] **Step 3: Review the final diff**

Check:

```bash
git diff origin/main...HEAD --check
git diff origin/main...HEAD --stat
git status --short
```

Expected: only the design, plan, implementation, tests, generated references, and Modal tutorial are changed.

- [ ] **Step 4: Push and open the PR**

Push `codex/node-globals-modal-tutorial` and open a PR summarizing the default-off compatibility flag, pre-evaluation bootstrap, Modal fix, and verification evidence.
