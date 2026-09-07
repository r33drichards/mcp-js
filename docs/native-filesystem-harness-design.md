# Direct native filesystem design

Status: proposed revision of [pi #1](https://github.com/r33drichards/pi/pull/1)
and [mcp-js #260](https://github.com/r33drichards/mcp-js/pull/260), reviewed
2026-09-07. This document does not describe implemented adapter behavior.

## Verified upstream state

Fetched `r33drichards/mcp-js` main: `04876586`.
Since our original base `299a26df`, main adds Docker release-binary packaging
([#264](https://github.com/r33drichards/mcp-js/pull/264)) and label-gated PR fuzzing
([#265](https://github.com/r33drichards/mcp-js/pull/265)). It does not add a
native filesystem operation API. The Node example still builds a shared library
and generates bindings locally; the CLI release binary is not that library.
Use the native Node workflow for ABI verification, not a released Docker image.
`Dockerfile.source` is the source-testing image path when Docker tests are used.

[#253](https://github.com/r33drichards/mcp-js/pull/253), composable hooks, and
[#263](https://github.com/r33drichards/mcp-js/pull/263), label-gated load tests,
were merged into `claude/fuzz-on-label-knnaj9`, not main. Its enclosing
[#261](https://github.com/r33drichards/mcp-js/pull/261) was closed without merging.
The replacement #265 contains only `.github/workflows/fuzz.yml`; it does not
carry the hooks or load-test changes onto main.
Hook design below is based on merge commit
`20d5d5bcb79cae4f7660ac1ad66216cc7ae6624e`, specifically
`site-docs/concepts/hooks.md`. It is a pending integration dependency.
Native npm packaging [#259](https://github.com/r33drichards/mcp-js/pull/259)
and our constructor [#260](https://github.com/r33drichards/mcp-js/pull/260)
are also still open. Do not treat their APIs or artifacts as available on main.

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

## Revision and verification sequence

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

Current PRs remain prototypes until these changes and native verification are
complete. The prior setup and limitations remain documented in `mcp-js.md`.
