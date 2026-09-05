# Hooks: the programmable effect boundary

JavaScript inside the sandbox has no host access of its own. Every effect it can cause — a `fetch()`, an `fs.writeFile`, a subprocess, a module import, an upstream MCP tool call — crosses the isolate boundary through exactly one place: an **operation**. Hooks make that crossing programmable.

For each operation category, an ordered **hook chain** runs around execution:

- **Pre hooks** see the operation's input and may **deny** it or **rewrite** it.
- **Post hooks** see the input and the output and may **deny** the result or **rewrite** the output.

Policies — the allow/deny gates documented in [Security policies](policies.md) — are not a separate mechanism. Internally a policy chain *is* a pre hook: it is appended as the **last** pre hook, so it always evaluates the effective (post-mutation) input. The `policies` config key is the compatibility spelling of a gate-only hook, and the long-term direction is for hooks to be the primary vocabulary.

```mermaid
flowchart LR
    G[guest JS<br/>fetch / fs / exec / …] --> P1[pre hook 1]
    P1 --> P2[pre hook 2]
    P2 --> POL[policy<br/>final pre hook]
    POL --> X[execute<br/>effective input]
    X --> Q1[post hook 1]
    Q1 --> Q2[post hook 2]
    Q2 --> R[result back to guest]
    P1 -. deny .-> D[JS error]
    P2 -. deny .-> D
    POL -. deny .-> D
    Q1 -. deny .-> D
    Q2 -. deny .-> D
```

Each hook receives what the previous hook produced, so chains compose left to right: a rewrite made by `pre[0]` is the input `pre[1]` sees, and the policy — running last — approves what will actually execute. A pre hook can rewrite a request *into* compliance; nothing can rewrite one out from under an approval.

## Configuration

Hooks are configured per operation in `--policies-json`, alongside (or instead of) `policies`:

```json
{
  "fetch": {
    "policies": [{"url": "file:///etc/policies/fetch.rego"}],
    "pre":      [{"url": "file:///etc/policies/fetch_hooks.js"},
                 {"url": "file:///etc/policies/fetch_hooks.rego"}],
    "post":     [{"url": "http://opa:8181", "policy_path": "mcp/fetch/post"}]
  },
  "filesystem": {
    "pre": [{"url": "file:///etc/policies/audit_fs_hooks.js",
             "capabilities": ["fs"]}]
  }
}
```

Each hook source is:

| Field | Meaning |
|---|---|
| `url` | `file://*.js` → JavaScript hook; other `file://` → Rego (regorus) file or directory; `http(s)://` → OPA-style REST endpoint |
| `rule` | Rego: the eval rule (default derives from the operation's policy rule, `.allow` → `.pre`/`.post`). JS: the global function name (default `pre`/`post`) |
| `policy_path` | Remote: the REST data path (default: the operation's policy path + `/pre` or `/post`, e.g. `mcp/fetch/pre`; note `mcp_tools` → `mcp/tools/pre`) |
| `timeout_ms` | JS: per-call bound, default 5000; expiry terminates the script and fails the operation closed |
| `capabilities` | JS: guest APIs granted to the hook isolate (`"fs"`, `"fetch"`) — see [Hook capabilities](#hook-capabilities) |

## The hook contract

A hook evaluates to one of three shapes, in any backend:

| Result | Meaning |
|---|---|
| *nothing* (undefined Rego rule, JS `undefined`/`null`, absent remote result) | **Abstain** — allow, change nothing |
| bare `true` / `false` | Allow / deny (pure policy behavior) |
| `{"allow": bool, "reason": "...", "input"\|"output": {...}}` | Deny with a reason, or replace the input (pre) / output (post) |

Abstention is what makes partial hooks compose: a hook only speaks when its condition matches, and silence costs nothing. Denials surface in the guest as thrown errors carrying the reason — `denied by pre hook (credentials in query string)` — and the first denial short-circuits the rest of the chain.

Pre hooks receive the same input document the operation's policies see (for fetch: `{url, method, headers, url_parsed}`; for filesystem: `{operation, path, destination, …}`). Post hooks receive `{"input": <effective input>, "output": <operation output>}` — Rego reads `input.input`/`input.output`; JS gets them as two arguments.

The same hook, in the two local backends:

```rego
package mcp.fetch

pre := {"input": object.union(input, {"url": u})} if {
    startswith(input.url, "http://")
    u := concat("", ["https://", substring(input.url, 7, -1)])
}
```

```js
function pre(input) {
    if (input.url.startsWith("http://")) {
        return { input: { ...input, url: "https://" + input.url.slice(7) } };
    }
}
```

## JavaScript hooks

A `file://*.js` source runs in its own V8 isolate on a dedicated worker thread — never a sandbox's isolate thread. The isolate is created lazily and kept **warm**, so top-level state in the hook file persists across calls (deliberately: counters, caches, circuit breakers); calls through one evaluator are serialized.

By default the isolate is **bare**: no `fetch`, no `fs`, no ops — a hook is pure computation over its arguments, exactly like a Rego rule. Hooks may be `async` (or return a Promise): the worker drives the isolate's event loop until the result settles.

Every call is bounded by `timeout_ms` and **fails closed** on expiry: running script is terminated through V8's thread-safe handle, and a call parked on pending I/O is abandoned by the worker's event-loop timeout. A gate that cannot produce a verdict denies — otherwise slowing a hook down would switch it off.

### Hook capabilities

A JS hook source can opt into pieces of the guest environment via `capabilities`, expressed with the **same APIs the sandbox sees**: `"fs"` installs the `fs.*` wrapper, `"fetch"` installs `fetch()` (plus `atob`/`btoa`). This is how observing hooks get side effects — the shipped `policies/audit_fs_hooks.js` audits every filesystem write to a log file:

```js
const LOG = "/var/log/mcp-js/fs-audit.log";
async function pre(input) {
    if (["writeFile", "appendFile", "rename", "remove"].includes(input.operation)) {
        await fs.appendFile(LOG, input.operation + " " + input.path + "\n");
    }
    // no return value: observe and abstain
}
```

Hook-issued operations are **ungated** — they run through no hook chain and no policy. The hook file is operator-trusted configuration (the same trust level as the policy files themselves), and gating its operations would recurse into the very chain the hook runs inside: the audit hook above would trigger itself on every `appendFile`, forever.

## Per-operation capabilities

Mutation is honored only where the executor can apply it; everywhere else the system fails closed rather than silently ignoring a hook:

| Operation | Input mutation applied | Post hooks |
|---|---|---|
| `fetch` | `url`, `method`, `headers` (`url_parsed` re-derived after every rewrite) | response `{status, headers, body, …}` |
| `filesystem` | `path`, `destination` (a hook that drops a required `destination` errors) | — |
| `subprocess` | `command`, `args`, `cwd`, `env` | `{code, stdout, stderr}` |
| `mcp_tools` | `server`, `tool`, `arguments` | tool result |
| `run_js_file` | `path` (re-canonicalized after rewrite) | — |
| `websocket`, `http2`, `modules`, `fs_snapshot` | gate-only — a mutating hook fails the operation | rejected at startup |

Operations with derived input fields keep them consistent through mutation: fetch re-parses `url_parsed` from a rewritten `url` before the next hook runs, and `run_js_file` re-canonicalizes a rewritten path — so the policy, which runs last, never sees a stale derivation.

The `operation` discriminator is likewise pinned: the executor performs the operation it was invoked for regardless of the JSON field, so a hook that rewrites `operation` (say `writeFile` → `readFile`) could only make the policy evaluate something other than what will run. Such a mutation fails the operation closed.

## Policies are hooks

The `policies` key is implemented *in terms of* hooks: `build_hook_chain` wraps the configured `PolicyChain` in a `Hook::Policy` variant and appends it as the final pre hook. A policy is precisely a pre hook that only ever answers with a boolean and never mutates. These two configurations gate identically:

```json
{"fetch": {"policies": [{"url": "file:///etc/policies/fetch.rego"}]}}
```

```json
{"fetch": {"pre": [{"url": "file:///etc/policies/fetch.rego",
                     "rule": "data.mcp.fetch.allow"}]}}
```

(The one behavioral difference today: multiple entries under `policies` combine under the chain's `mode` — `all`/`any` — while `pre` hooks always run in order with first-denial short-circuit. An `any`-mode policy chain has no direct `pre` spelling yet.)

The direction of travel is to phase out the separate policies vocabulary: `policies` remains supported as the compatibility spelling, but new gating, rewriting, and observing behavior should be written as hooks, and the deny messages already distinguish `denied by policy` from `denied by pre hook (reason)` only for continuity.

## Design direction: composable effect boundaries

Because every sandbox effect already crosses through exactly one operation seam, the hook chain generalizes further — toward the model SQLite uses for its VFS layer, where each shim implements the same interface, wraps the next one down, and the operating system itself is just the default bottom layer.

Today's chain is flat: `pre* → execute → post*`, with `execute` fixed. The composable-boundary form makes each hook a **layer around the rest of the stack**:

```text
handle(input, next) -> output
```

A layer can call `next(input')` zero, one, or many times — and that one change subsumes everything the flat chain does while adding what it cannot express:

| Capability | Flat chain (today) | Layered (direction) |
|---|---|---|
| Deny / rewrite input | ✅ pre hook | ✅ don't call `next`, or call it with `input'` |
| Deny / rewrite output | ✅ post hook | ✅ transform `next`'s return |
| Short-circuit with a synthetic result (mock, cache hit) | ❌ | ✅ return without calling `next` |
| Retry / fallback | ❌ | ✅ call `next` again |
| Pair state across one call (timing, request↔response correlation) | ❌ pre and post are separate hooks | ✅ one closure sees both sides |
| Swap the executor itself (virtual fs, recorded network) | ❌ `execute` is fixed | ✅ the real executor is just the innermost layer |

In that model the *boundary itself* is a hook: the terminal executor — the real HTTP client, the real filesystem — is simply the default innermost layer, replaceable in configuration the way SQLite swaps its bottom VFS. A policy is the degenerate layer `if allow(input) { next(input) } else { deny }`; an audit log is a layer that calls `next` and appends a line either side; a record/replay harness is a layer that never calls `next` at all.

None of the layered form is implemented yet — it is the design direction this system was shaped for. The current contract was chosen to be forward-compatible with it: every existing hook (abstain / deny / rewrite) maps mechanically onto a layer, so migrating the engine underneath does not have to break a single configured hook.

## Worked examples

The repository ships three tested example hook files:

- `policies/fetch_hooks.rego` — https upgrade, query-string credential refusal, response-header scrubbing, in Rego
- `policies/fetch_hooks.js` — the same hooks in JavaScript
- `policies/audit_fs_hooks.js` — capability-bearing write-audit logging

and an end-to-end test suite (`server/tests/hooks_e2e.rs`) that drives real guest `fetch()`/`fs.*` calls through mutating, denying, and auditing chains.
