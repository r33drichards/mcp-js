# Dynamic JavaScript HTTP functions — implementation plan

## Authorized implementation update

Robert requested implementation and table-based integration tests, then explicitly rejected additional feature restrictions. The implementation therefore inherits the existing transport authentication rules, without an operator allowlist, new roles, or owner-based restrictions. This supersedes the separate-principal/authority gates proposed below; existing runtime policies and deployment limits still apply. Deployment remains out of scope.

Work in progress: `server/src/http_functions.rs` contains dispatcher, conditional durable registry, Engine adapter, response validation, and table-driven HTTP tests. Validation has not passed yet; do not treat this as complete or deployable. Cluster failover/quorum and authorization regression evidence remain outstanding.


## Scope and stack

Plan-only PR stacked on #284, base branch impl/scheduler-apalis, inspected at 8677390325b1769a715bd889bcaa22bd1167cf9f. No implementation or deployment included. Keep this PR based on that branch until #284 merges; then rebase/retarget to main and verify only this plan remains in the diff.

Outcome: an authenticated operator can dynamically register, replace, list and delete named JavaScript HTTP handlers without restarting mcp-js. Requests run an immutable handler revision in the existing Engine/V8 environment used by ordinary and scheduled executions, not a second unrestricted runtime.

## Existing seams

- server/src/api.rs: Axum routes, ExecutionRequest integration, body limits, execution status and result mapping.
- server/src/engine: reuse Engine isolate creation, capabilities, policy hooks, imports, heap/time limits and lifecycle.
- #284 server/src/scheduled_worker.rs: Runtime abstraction, dedicated mode, admission and one-use launch capability.
- server/src/scheduled_protocol.rs: versioned payload and worker/authority binding.
- scheduler-core and scheduler-runner: durable scheduling and fenced admission. HTTP functions are NOT schedules: do not fabricate occurrences or reuse scheduler permits.

Keep scheduled-only worker mode scheduled-only: never mount function routes there. Ordinary server mode gains opt-in routes. Do not change #284 checkpoint/wire schemas or weaken its admission/Unknown semantics.

## Proposed API

Freeze proposed fields with contract tests.

Management, authenticated and explicitly authorized:

- PUT /api/http-functions/v1/definitions/{name}: create/replace. Require If-None-Match: * for creation and If-Match with current revision for replacement; stale revision returns 412.
- GET /api/http-functions/v1/definitions and GET .../{name}: bounded metadata. Source retrieval requires separate read-source authority.
- DELETE .../{name}: revision precondition and committed tombstone. New admissions stop; already admitted work may finish.

Invocation: fixed dispatcher /functions/v1/{name}, optionally with a suffix wildcard passed as request data. Initial version has exact configured methods and one named mount, not arbitrary regex routing. Reject reserved names, traversal, encoded slash ambiguity and invalid methods. Never override /api, /mcp, scheduler, health or discovery routes. Unknown function returns 404; wrong method returns 405.

Example registration:

    {
      "version": 1,
      "methods": ["POST"],
      "source": "async function handler(request) { return {status: 200, headers: {'content-type': 'application/json'}, body: JSON.stringify({ok: true})}; }",
      "export": "handler",
      "limits": {"timeout_seconds": 30, "heap_mb": 64},
      "authority_profile": "operator-reviewed-profile"
    }

Export is a validated identifier resolving to a callable. Persist source and metadata, not captured closures, live isolates or function pointers. Invocation callers supply data, never code. TypeScript may reuse the existing transpiler. Validate syntax without executing top-level code or granting capabilities during registration. Top-level source executes only inside an authorized, limited invocation.

## Execution environment and HTTP contract

Pin one immutable revision per admission. Submit a fresh Engine execution with the same supported MCP, fetch, filesystem, imports and artifact facilities as configured for ordinary execution. Extract shared setup instead of copying run_js initialization. Preserve policy/hook ordering, including authorization before secret injection. No implicit shared globals, reused heaps or writable cross-principal state.

Expose structured request input: method, normalized relative path, bounded query multimap, filtered headers and base64 raw body. Prefer a structured Engine input binding. If wrapping source is necessary, serialize request as JSON data, never concatenate untrusted input as executable code. Do not forward authorization/cookie headers by default. Identity is server-derived, never a session/logging tag or caller-supplied header. Additional forwarded headers require reviewed configuration.

Return a structured response: integer status, bounded header map and exactly one UTF-8 body or base64 binary body. Capture structured result independently from console output; logs are not the response body. Validate status and headers, reject CRLF/hop-by-hop headers, calculate content length server-side and enforce response size. HEAD suppresses bodies; bodyless statuses follow HTTP semantics. No streaming, WebSockets, SSE or raw socket takeover in v1.

Respond only after successful execution and validation. Proposed errors: 400 invalid request, 401/403 authorization, 404 missing, 405 method, 409 capacity, 412 stale revision, 413 oversized input, 500 generic handler/result failure, 503 unavailable/quorum loss, 504 timeout. Never expose source, stack traces, injected secrets or filesystem paths. Request IDs link redacted diagnostics to execution records.

## Security

Disabled by default; explicit enablement and authorization required. Separate management and invocation capabilities. Initially require invocation authentication too; anonymous/public routes need a later explicit design, not missing middleware.

Bind definitions to verified owner/namespace and operator-approved execution authority. Registrants cannot choose arbitrary policies, credential references, upstream MCP servers or filesystem roots. Effective permissions intersect deployment policy, definition profile and invocation principal authority. Implement principal binding before multi-tenant exposure. If auth cannot supply a stable principal, keep the feature operator-only and fail closed; session is not identity.

Authenticate before buffering bodies or submitting execution. Bound source/registry sizes, invocation input/output, per-principal quotas, global/per-function concurrency, queues, timeout and heap. Clamp limits to operator maxima. No credential values in definitions, Raft records, API responses, exported source or logs. Existing host-scoped secret injection remains server-side and policy-controlled.

## Registry, persistence and consistency

Use existing Raft state-machine infrastructure after inspecting #284 storage seams; separate registry keyspace/schema from scheduler checkpoints. Single-node mode uses the same logical revision/CAS contract. Publish cached routes only after commit; failed writes/quorum loss cannot publish speculative definitions.

Clustered invocation requires a linearizable read/barrier, or leader dispatch with equivalent guarantees, before new admission. Do not admit indefinitely stale follower-cache definitions after committed replacement/deletion. Each admitted request retains its revision and authority. Persist tombstones/revision counters to prevent ABA after restart. Invalid/unsupported definitions fail closed on startup without executing source. Bound registry storage and define safe tombstone compaction.

HTTP retries are new requests; no automatic replay after crash, disconnect, timeout, failover or ambiguous Engine submission. Effects may outlive client disconnect; propagate cancellation through existing Engine facilities where possible, without claiming rollback or guaranteed cancellation. No exactly-once claim. Future idempotency must bind principal, revision and payload digest and retain ambiguous outcomes; it is out of v1 scope.

## Implementation sequence

1. Inspect shared Engine submission/result APIs and auth principal extraction. Record adapter gaps; freeze schemas, error mapping and consistency semantics with tests.
2. Add definition validation and versioned registry commands/CAS/tombstones (proposed http_functions/{types,registry}.rs). Test durability and clustering.
3. Extract common Engine request construction without changing /api/exec, run_js or scheduler behavior. Add structured request/result adapter and response validation.
4. Add opt-in management and fixed-prefix dispatcher with independent authorization, quotas and body limits. Keep scheduled-only router unchanged.
5. Add fault/integration tests, OpenAPI documentation and runnable registration/curl examples. Deployment requires separate approval.

## Acceptance evidence

- Live registration, conditional replacement, metadata listing and deletion work without restart. Unauthorized management/invocation and cross-owner access fail closed.
- JSON/binary bodies, duplicate query keys, filtered headers, HEAD/bodyless status handling and method/path errors have deterministic tests. Reject malformed base64, invalid status, CRLF and oversized input/output.
- Real Engine tests prove async handler execution, policy-equivalent capabilities/imports, host-scoped header protection, timeout/heap limits, fresh-global isolation and safe request serialization. Include source/input injection probes.
- Concurrent replacement pins exactly one revision; deletion prevents subsequent admissions while existing requests follow documented semantics.
- Restart and leader failover preserve registry/CAS/tombstones. Partitions/quorum loss cannot publish changes or admit stale routes. Test disconnect/ambiguous submission without replay.
- Regression tests prove scheduled-only mode exposes no function, legacy /api/exec or MCP routes; scheduler admission tests pass unchanged.
- Run project checks identified in Cargo manifests/CI, including relevant server and scheduler-core/runner suites, formatting and diff audit. Add focused CI and real HTTP lifecycle tests. Record exact commands/results in the implementation PR.

Done means implemented behavior and evidence for every criterion, not compilation alone. If shared Engine structured results, stable principals or linearizable lookup are unavailable, stop with the precise gap and required decision. Do not substitute an unrestricted runtime, weaken authentication or bypass scheduler admission.

## Known #287 baseline test failures (2026-10-25)

Despite its plan-oriented title, #287 already contains the partial HTTP-function
implementation from `ca9b5338`. The scheduler-hardening integration leaves that
implementation and its tests unchanged. A fresh full server-library run on the
integrated #287 code passed **277 tests and failed two**:

- `http_functions::tests::table_driven_handler_results`: HTTP 500 instead of 201.
- `http_functions::tests::table_driven_http_lifecycle_and_real_engine`: HTTP 500
  instead of 200.

Both failures were reproduced on untouched #287 head
`d18df1010ad20ea5b074c943933eca27726ef520`, before the new hardening merge:

```sh
git worktree add --detach /tmp/mcp-287-baseline d18df1010ad20ea5b074c943933eca27726ef520
cd /tmp/mcp-287-baseline
cargo test --locked -p server --lib http_functions::tests::table_driven -- --test-threads=1
```

That targeted baseline run produced **0 passed, 2 failed, 271 filtered out**.
To reproduce the full integrated #287 check, run in a fresh checkout of that
branch:

```sh
cargo test --locked -p server --lib -- --test-threads=1
```

Use a fresh Cargo target directory (or clean the `server` package) when comparing
checkouts; a shared target cache can reuse an older local-package test binary.
The partial Engine returns no structured handler result; #293 supplies that
completion. No tests were disabled or assertions weakened to hide these baseline
failures, and no completion code was moved backward into #287. This is a known
intermediate-stack limitation, **not a clean full-server-test pass for #287**.
Validate the completed HTTP-function layer on #293 and its descendants.
