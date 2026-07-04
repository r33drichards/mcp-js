# Formal verification pipeline for mcp-js (Charon + Aeneas + Leanstral workflow)

This directory sets up the [Leanstral 1.5](https://mistral.ai/news/leanstral-1-5)
bug-finding pipeline against the pure logic in this repository:

> Aeneas translates Rust code to Lean, while Leanstral infers the user intent and
> generates correctness properties from the code. Leanstral then attempts to
> prove each property in four attempts. If they all fail, it tries to prove the
> negation instead, also with four attempts.

It found **one real bug**: a reachable panic in the fetch-header host matcher.

## What was set up

| Tool | Role | Status |
|------|------|--------|
| **Mistral Vibe CLI** (`mistral-vibe` 2.19.0) | Agent runner | Installed (`uv tool install mistral-vibe`); Lean agent + `lean-lsp` MCP server configured in `~/.vibe/config.toml` (equivalent to running `/leanstall`) |
| **Lean LSP MCP** (`lean-lsp-mcp`) | Lean goal/error introspection | Configured as a stdio MCP server (`uvx lean-lsp-mcp`) |
| **Charon** (`19e3f85`) | Rust → LLBC extractor | Built from source with `cargo` |
| **Aeneas** (`45061fa`) | LLBC → Lean translator | Built from source; `aeneas.nix` reproduces it offline from a pinned nixpkgs |
| **Leanstral `leanstral-1-5`** | Prover model | **Endpoint `api.mistral.ai` is not reachable from the build sandbox and no `MISTRAL_API_KEY` is set** — see "Leanstral model access" below |

## The pipeline

1. **Extract translatable Rust.** deno_core / V8 / axum / tokio are outside the
   Rust subset Aeneas supports, so the pure, panic-relevant logic was ported
   into a small standalone crate (`crate/`), staying byte-for-byte faithful to
   the originals in `server/`. Three functions were ported:
   - `host_matches` ← host-pattern half of `HeaderRule::matches`
     (`server/src/engine/fetch.rs:164`)
   - `parse_memory_size` ← `parse_memory_size` (`server/src/main.rs:983`)
   - `validate_wasm_name` ← `validate_wasm_name` (`server/src/main.rs:1001`)

   Fidelity is pinned down by `crate/tests/differential.rs`, which runs the port
   and a **verbatim copy** of each original on the same inputs and asserts they
   agree — including that both panic on the `"*"` pattern (4/4 tests pass).

2. **Charon** extracts `mcpjs_verify.llbc` (`charon cargo --preset=aeneas`).

3. **Aeneas** translates it to Lean (`lean/McpjsVerify/`).

4. **Leanstral workflow** — infer intent, generate a correctness property per
   function, try to prove it, and on failure try to prove its negation. The
   property statements are in `lean/McpjsVerify/Properties.lean`.

`./run-pipeline.sh` reproduces steps 1–3.

## The bug — reachable panic in the fetch-header host matcher

`server/src/engine/fetch.rs:164`:

```rust
let pattern = self.host.to_lowercase();
let host = request_host.to_lowercase();
if let Some(suffix) = pattern.strip_prefix('*') {
    // "*.github.com" matches "api.github.com" and "github.com"
    host == pattern[2..] || host.ends_with(suffix)   // <-- panics when pattern == "*"
} else {
    host == pattern
}
```

- `HeaderRule::new` (`fetch.rs:107`) rejects only an **empty** host, so a rule
  with `host = "*"` is accepted.
- `apply_header_rules` (`fetch.rs:293`) calls `matches` for **every** outgoing
  fetch, over every configured rule.
- When the pattern is exactly `"*"` (one byte), `strip_prefix('*')` succeeds and
  the code evaluates `pattern[2..]` on a length-1 string → **`byte index 2 is
  out of bounds`**, panicking the request-handling task.

`host = "*"` is a natural thing to write meaning "inject this header on all
hosts", so this is easy to hit in practice. The unique trigger is the pattern
`"*"` itself (any `*`-prefixed pattern of length ≥ 2 is fine).

**In the translated Lean** (`lean/McpjsVerify/Funs.lean`, `model.host_matches`)
the same step appears as `Vec.index (RangeFrom {start := 2}) pattern`, whose
Aeneas precondition is `2 ≤ pattern.length`. For `pattern = "*"` that is
`2 ≤ 1`, so the computation reduces to `fail .panic`. This is exactly why the
"host_matches never panics" property is **unprovable** and its **negation**
(`host_matches_can_fail`) holds.

### Suggested fix

Strip only the `*` rather than two bytes, or special-case a bare `"*"`:

```rust
if let Some(suffix) = pattern.strip_prefix('*') {
    // suffix is pattern without the leading '*'
    host == suffix.strip_prefix('.').unwrap_or(suffix) || host.ends_with(suffix)
}
```

(Pick whichever matches the intended wildcard semantics; the key point is to
never index `pattern[2..]` without checking `pattern.len() >= 2`.)

## Leanstral model access

The blog workflow's prover step calls the hosted `leanstral-1-5` model at
`https://api.mistral.ai/v1`. In this environment that host is blocked by the
egress proxy (connections return `000`) and no `MISTRAL_API_KEY` is configured,
so the model itself could not be invoked here. The prove/disprove **methodology**
was therefore carried out directly, and the property that the model would have
targeted is recorded in `Properties.lean`. To run the real model:

1. Set `MISTRAL_API_KEY` and ensure `api.mistral.ai` is reachable.
2. `vibe --agent lean` (the Lean agent + `lean-lsp` MCP are already configured).
3. Point it at `lean/` and ask it to discharge `Properties.lean`.

## Machine-checking the Lean

`Properties.lean` currently uses `sorry` for the proof/disproof bodies. Checking
them requires the Aeneas Lean library (which pulls in mathlib) under
`lean/lakefile.lean`; that toolchain could not be built in this sandbox
(Lean-release downloads and the mathlib build cache are both proxy-blocked). The
underlying claims are instead verified concretely by the Rust differential
harness in `crate/tests/differential.rs`.

## Layout

```
verification/
├── README.md              — this file
├── run-pipeline.sh        — reproduce extract → Charon → Aeneas
├── aeneas.nix             — offline Nix build of Aeneas (+ charon-ml, easy_logging)
├── crate/                 — standalone Rust crate: ports + differential tests
│   ├── src/model.rs       — byte-level ports of the mcp-js functions
│   └── tests/differential.rs — ports vs verbatim originals (4/4 pass)
└── lean/                  — Aeneas output + property statements
    ├── lean-toolchain     — leanprover/lean4:v4.30.0-rc2
    ├── lakefile.lean
    └── McpjsVerify/
        ├── Funs.lean          — translated function bodies
        ├── Types.lean
        ├── FunsExternal.lean
        └── Properties.lean    — correctness properties (the workflow output)
```
