# Pre-run scripts

Two flags let an operator run their own JavaScript/TypeScript before the code
submitted through `run_js`, without rebuilding the server:

- `--init-script` — runs **once per heap lineage**: before an execution whenever
  the isolate lacks the init marker (`globalThis.__mcpV8InitDone`). On success
  the marker is set and, in stateful mode, baked into the resulting heap
  snapshot, so descendants of that heap skip it.
- `--pre-run-script` — runs before **every** execution, right before the
  submitted code, including on snapshot-restored isolates.

Either, both, or neither may be set. When both fire, the order is:
init script → pre-run script → user code.

## Value syntax: inline code or `@file`

Same syntax as [`--instructions`](customize-mcp-surface.md):

| Value | Meaning |
|-------|---------|
| `"globalThis.x = 1"` | Used literally as the script |
| `@./init.js` | Read the script from the file `./init.js` |
| `@@code` | Literal leading `@` — produces `@code` |

Environment variables `MCP_V8_INIT_SCRIPT` / `MCP_V8_PRE_RUN_SCRIPT` and the
config-file keys `init_script` / `pre_run_script` work too. TypeScript is
accepted; types are stripped once at startup (an invalid script fails startup,
not each execution).

```bash
# Seed every session with helpers, and log before every run
mcp-v8 --http-port 8080 \
  --init-script @./bootstrap.js \
  --pre-run-script "console.log('execution starting')"
```

## Scripts are ES modules

Both scripts run as ES modules, so `import` (including `node:` built-ins and —
with `--allow-external-modules` — `npm:`/`jsr:`/URL specifiers) and top-level
`await` work. Module bindings do **not** leak into other code: expose values
explicitly via `globalThis`.

```js
// bootstrap.js
import path from "node:path";
globalThis.helpers = {
  join: (...parts) => path.join(...parts),
};
```

An `npm:` import in the init script is fetched once per new heap lineage and
baked into the snapshot; in the pre-run script it is on the critical path of
every execution.

## The init marker

- The marker is checked before each execution; when absent, the init script
  runs. That includes heaps created **before** the flag was set (or by a server
  without it) — they are initialized on their next run, exactly once.
- The marker is set only after the init script evaluates **successfully**. A
  throwing init script fails the execution (error prefixed with
  `init script failed:`), no heap is persisted, and the script retries on the
  next run.
- Stateless mode has no snapshots, so the marker is always absent and the init
  script runs before every execution.
- The marker is hidden from enumeration (`Object.keys(globalThis)`), but user
  code can read, set, or `delete globalThis.__mcpV8InitDone` to force a re-init
  on the lineage's next run. It is a convenience latch inside the session's own
  trust domain, not a security boundary, and it is presence-only — changing the
  configured script does not re-run it on already-initialized heaps.

## Failure, environment, and limits

- A script error fails the execution with a prefixed error
  (`init script failed: ...` / `pre-run script failed: ...`); the submitted
  code does not run.
- Scripts run **after** [sandbox hardening](../concepts/js-execution.md), in
  the same environment as user code — the same globals, policies, and gated
  capabilities apply.
- Script time counts against the execution timeout, and script allocations
  count against the heap memory limit.
- `console.*` output from scripts appears in the execution's console output.
