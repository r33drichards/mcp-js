# Direct native filesystem design

Status: implemented on `main` of [pi](https://github.com/r33drichards/pi)
(merged as r33drichards/pi#2) and this repository as of 2026-09-07.
This document records the design; `node/README.md` documents the shipped native
filesystem API.

## Upstream state

Main merged composable pre/post hooks
([#266](https://github.com/r33drichards/mcp-js/pull/266)) on 2026-09-07, so the
hook integration base this design waited for exists and the integration branch
is rebased onto it. On that branch:

- `Engine::create_with_filesystem` builds the filesystem `OperationPolicies`
  with `build_hook_chain`, so `pre`, `stack`, and rewrites apply; a
  configuration with no policies, pre hooks, or stack is rejected.
- `fs::FsService` is the shared operation service below both the Deno ops and
  the native `Engine::fs_*` methods; guest wire messages are unchanged and the
  hooks end-to-end suite passes against it.
- The native methods carry bytes as `bytes` and fail with
  `RuntimeError::FileSystem { kind, message }`.
- Ranged reads (`fs_read_file_range`) give the harness a bounded line reader.
  Native reader/writer objects with explicit close are not implemented.
- Overlay-backed engines are rejected by the native methods; the overlay must
  run on the isolate's current-thread runtime.

Native npm packaging ([#259](https://github.com/r33drichards/mcp-js/pull/259))
is still separate and open; bindings are generated locally or in CI.

## Problem and target boundary

The current adapter turns file operations into guest JS, then sends their
results through JSON console output. A binary read becomes bytes -> numeric
array -> JSON text -> numeric array -> bytes. Large files are fully buffered.

Replace that bridge with a native filesystem handle sharing the engine's
filesystem namespace and operation authority:

```text
V8 fs.* -> Deno op wrapper -----+
                               +-> shared filesystem operation service
Node file tools -> UniFFI ------+     -> hook/policy evaluation
                                     -> effective path/destination
                                     -> host or session overlay backend
```

Keep `run_js` for model-authored JavaScript. Native file tools must not execute
generated JS or use Node fs as a fallback. Native handles are bound to the
configured engine/session, headers, filesystem view, and lifecycle; callers
cannot substitute a different authority on each chunk. Names below describe
capabilities, not existing generated binding methods.

## Hook-aware semantics

Reuse the same operation implementation below the Deno wrapper, not just its
raw I/O calls. Once #253 reaches the integration base:

- Run pre hooks in order and evaluate policy against rewritten effective input.
- Apply rewritten source and destination paths to the actual I/O. Pin the
  operation discriminator so a write cannot be authorized as a read.
- Preserve filesystem gate-mode stacks: exactly one terminal descent, no
  synthetic return data or retries/short-circuits, no virtual filesystem executor.
  Filesystem post-output transformation is not supplied by the reviewed hooks.
- Preserve fail-closed denial and timeout behavior, including configured audit
  hooks. Do not initialize a separate policy-only chain for native calls.
- Operator-trusted hooks may have ungated fs/fetch capabilities to avoid
  recursion. Those capabilities must never leak into the ordinary native file
  handle exposed to the harness.

Constructor configuration should reuse the upstream filesystem operation config,
including hooks/stacks when supported. Our current mandatory nonempty `policies`
list is too narrow for explicit hook-only configurations. Missing configuration
must not silently grant filesystem authority; explicit configured authority must
be interpreted consistently with upstream, not through an adapter-specific gate.

## Binary and bounded I/O

Expose typed metadata, directory listing, mkdir, rename, remove, symlink-aware
path operations, byte reads/writes, and structured filesystem errors via UniFFI.
Do not encode file bytes in JSON, base64, or console output. UniFFI may copy byte
buffers: verify generated binding types and bound chunk sizes rather than
claiming zero-copy.

For large files, add native reader/writer objects with bounded chunk operations
and explicit close. Reuse existing native overlay writer machinery where its
semantics fit; guest `fs.createWriteStream` is not itself a foreign-call API.

Define and test before exposing streams:
- Reader offset, chunk-size limits, EOF, cancellation, and post-close errors.
- Writer overwrite/append mode, partial-write reporting, close versus abort,
  and overlay snapshot publication. Do not promise atomic host writes unless
  explicitly implemented.
- Authorization lifetime. Evaluate hooks/policy on opening the logical operation
  and bind the effective target to its handle; choose and document whether later
  chunks reauthorize. Never re-resolve a rewritten path independently per chunk.
- Engine shutdown closes outstanding handles and rejects new work. Cancellation
  must not report rollback of already performed writes.

Avoid holding a shared filesystem lock while invoking hooks that may perform
I/O. Keep session/overlay identity identical between `run_js` and native tools.

## Harness changes

Retain `FileToolContext` and the separate `JavaScriptRuntime` contract. Replace
McpJsExecutionEnv's generated-source evaluator with direct typed native calls.
Introduce a bounded read capability in the harness filesystem contract, then
make `read` use it: currently the tool reads the entire file before line slicing.
Use a small image-signature probe and a separately bounded image path. Decode
text incrementally across UTF-8 chunk boundaries; stop when line/byte limits
are reached, including a single oversized line. Define offset scanning costs.

`edit` can still require complete content for exact replacement, but must have
an explicit size bound rather than implying arbitrary-size streaming edits.
Output streaming for `run_js` is separate work: use execution IDs, bounded
console pages, and cancellation APIs rather than assuming direct fs solves it.
Verify the foreign-thread Tokio execution path before using submit APIs directly.

## Verification sequence and status

1. Wait for, or explicitly select, the hook integration base without silently
   rebasing shared PRs onto an unmerged feature branch.
2. Extract the common native filesystem service and expose byte/metadata calls;
   verify guest and UniFFI parity for allows, denies, rewrites, and audit events.
3. Add bounded native readers/writers and test cancellation, cleanup, memory
   bounds, host/overlay visibility, and structured errors.
4. Replace the pi adapter and update read-tool behavior. Test UTF-8 boundaries,
   binary data, huge lines/files, path rewrites, and no JS-evaluation/Node-fs
   fallback during file calls.
5. Build real native bindings and run cross-language integration tests in CI.
   Mock adapter tests alone are not native integration proof.

Status: steps 1 through 4 are implemented on the integration branches. Step 5
is `.github/workflows/pi-harness-e2e.yml`, which builds the shared library,
generates bindings, checks out pi, and runs its file tools and `run_js` against
the real engine (`node/pi-harness/tests/harness.test.ts`). The pi-side guide is
[packages/agent/docs/mcp-js.md](https://github.com/r33drichards/pi/blob/main/packages/agent/docs/mcp-js.md).
