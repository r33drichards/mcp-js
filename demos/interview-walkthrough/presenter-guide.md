# Presenter guide: alert to dashboard in 45 minutes

Audience: interviewers evaluating Robert’s work (confirmed). Challenge brief and date remain unknown. No authorship or rationale is inferred from repository ownership.

Base: [`fe5b613db0afcc9cf20d7ca1ef659c914a0b5008`](https://github.com/r33drichards/mcp-js/commit/fe5b613db0afcc9cf20d7ca1ef659c914a0b5008). This is exactly the earlier COW reference commit, not an assumed unchanged newer version.

Before presenting, fill in: **my contribution**, **requirement I owned**, **alternatives I actually considered**, **decision evidence**, **what I would change with today’s requirements**. The source does not answer these personal questions.

Use slides.html, README.md for launch, verification.md for honest status, and architecture-reference.md for detailed implementation/Q&A. Main workload: pinned Astronomy Shop. No remediation.

| Clock | Segment | Minutes |
|---|---|---:|
|00–03|Problem and launch investigation|3|
|03–12|Compose architecture while Pi works|9|
|12–18|Dashboard handoff and policy|6|
|18–28|Paired forks and explicit merge|10|
|28–35|Node failure and recovery|7|
|35–39|Tradeoffs and limits|4|
|39–45|Q&A — six reserved minutes|6|

**Total: 45 minutes.** Stop live troubleshooting after 30 seconds; preserve Q&A.


## 00–03 — An alert is a question.

**What:** Introduce the alert and your actual contribution; start the approved Pi investigation early. Keep actual status visible while explaining the Compose services.

**How:** Pi → bound MCP adapter → mcp-js → official Grafana MCP → Grafana → Prometheus. Browser checks the rendered result.

**Why:** A reproducible investigation keeps queries, files and publication decisions inspectable. This is the demo’s framing, not an assertion of original author intent.

**Notes:** Open with your actual role and the requirement you owned. The brief is still unknown. Pin the implementation to fe5b613. Operator-injected application faults and cluster outages are separate experiments. Code defaults heap_store/fs_store to none despite README default-state wording. Compose reference: compose.astronomy.json; generated .state/compose.json is the launch artifact. Return to real dashboard status when ready; no timer simulates progress.

[Source](https://github.com/r33drichards/mcp-js/blob/fe5b613db0afcc9cf20d7ca1ef659c914a0b5008/server/src/cli.rs#L196)

## 03–06 — Read the alert.

**What:** Accept DemoCpuDrop for nab-ec2-replay. Reject another service. Store the alert and its source.

**How:** The adapter validates a small JSON envelope. It chooses fixed local resources; alert annotations do not become commands, paths or PromQL.

**Why:** A narrow demo scope makes investigation reproducible and avoids turning an alert body into executable authority. Inference: reduce capability surface.

**Notes:** Use the alert panel. Astronomy Shop 2.1.3 is pinned. Stock OTLP logs fan out to added Loki for the official Grafana MCP log tool. Full workload execution is not yet verified. The demo investigates, it does not repair anything. During generation open compose.astronomy.json. Follow query, grafana-mcp, browser, node1/node2/node3, objects and publisher. Do not fake a timed completion. Compose reference: compose.astronomy.json; generated .state/compose.json is the launch artifact. Return to real dashboard status when ready; no timer simulates progress.

[Source](https://github.com/r33drichards/mcp-js/blob/demo/interview-walkthrough/demos/interview-walkthrough/alerts.py)

## 06–09 — Ask the metrics.

**What:** Query demo_cpu_utilization_percent, inspect nonempty samples, and save the response under /work/scratch/.

**How:** mcp.callTool("grafana", "query_prometheus", …) uses the official server and provisioned demo-prometheus datasource. The query result goes into a COW snapshot.

**Why:** The route preserves datasource configuration and observable tool results. No invented tool names and no direct Prometheus shortcut in the agent query.

**Notes:** Show the exact tool schema and arguments. Read isError, terminal execution status and returned data. Pi model calls were not made; deterministic integration checks use the same MCP path. Never imply deterministic checks are an agent recording. Compose reference: compose.astronomy.json; generated .state/compose.json is the launch artifact. Return to real dashboard status when ready; no timer simulates progress.

[Source](https://github.com/r33drichards/mcp-js/blob/fe5b613db0afcc9cf20d7ca1ef659c914a0b5008/server/src/engine/mcp_client.rs#L830)

## 09–12 — Execution ends. State is explicit.

**What:** Follow submission, execution UUID, console output, fs root and session history. Identify which pieces survive.

**How:** Tokio task → V8 semaphore → blocking worker → deno_core isolate → snapshot flush. Heap and fs persistence are independent. Execution status is in memory; console output is in sled.

**Why:** Isolation and persistence solve separate problems. Source implements separate state axes; this allows fs persistence with a fresh heap. Tradeoff: no transaction over all external effects.

**Notes:** Timeout requests V8 termination; it is not a whole-request hard deadline. Warming/queueing precede the timeout selection. OS sandbox is optional process-wide defense. Cite the print-alias fix as a concrete security lesson without claiming authorship. Compose reference: compose.astronomy.json; generated .state/compose.json is the launch artifact. Return to real dashboard status when ready; no timer simulates progress.

[Source](https://github.com/r33drichards/mcp-js/blob/fe5b613db0afcc9cf20d7ca1ef659c914a0b5008/server/src/engine/mod.rs#L2458)

## 12–15 — Make the handoff visible.

**What:** Stage dashboard JSON in /work/dashboards/. Explicitly export one selected root. Verify the browser sees time-series panels with data.

**How:** A separate publisher materializes the virtual file, validates its identity, writes a temporary host file, fsyncs and renames it. Grafana polls the shared provisioned directory.

**Why:** COW storage is not a host mount. The explicit boundary prevents two investigation branches from racing writes into the live dashboard.

**Notes:** Show the root, source path, SHA-256, Grafana URL, and browser evidence. Provisioning API success alone does not establish visible graphs. If the browser is unavailable, label the handoff incomplete rather than showing an illustrative graph as live. Compose reference: compose.astronomy.json; generated .state/compose.json is the launch artifact. Return to real dashboard status when ready; no timer simulates progress.

[Source](https://github.com/r33drichards/mcp-js/blob/fe5b613db0afcc9cf20d7ca1ef659c914a0b5008/server/src/engine/fs_mount.rs#L337)

## 15–18 — Allow one path. Deny another.

**What:** Read/write /work/scratch/ and /work/dashboards/. A write to /work/protected/ fails. Existing snapshot remains readable.

**How:** FsService gates effective operation/path before dispatch to the overlay. Demo Rego checks operation plus namespace and rejects traversal components.

**Why:** Per-operation policy provides a boundary more precise than handing the guest the host filesystem. Source comments document this gate; Robert supplies the original decision history.

**Notes:** Policy input is operation, not op. Show failed status/error rather than treating any exception as proof of policy denial. No passthrough. Directory creation is implicit in virtual mode. Additional hardening is opt-in. Compose reference: compose.astronomy.json; generated .state/compose.json is the launch artifact. Return to real dashboard status when ready; no timer simulates progress.

[Source](https://github.com/r33drichards/mcp-js/blob/fe5b613db0afcc9cf20d7ca1ef659c914a0b5008/server/src/engine/fs.rs#L143)

## 18–23 — Fork two views of the same bytes.

**What:** Both investigations start at B. A and T change their own files. Read B again to prove it is unchanged.

**How:** SessionMount pins base_id and records Data/Whiteout entries. Push rewrites changed tree paths; chunks and unchanged subtrees are shared.

**Why:** Commit 4d6e446 documents recursive trees, lazy traversal and copy-by-reference. Dedup is by content. There is no filesystem commit ancestry in a root.

**Notes:** Bincode BTreeMap directory nodes use BLAKE3; <=64KiB files inline; larger files use FastCDC 256KiB/1MiB/4MiB with optional zstd. Caveat: execution prefetch traverses the base, so no unmeasured “instant fork” or O(touched) startup claim. Compose reference: compose.astronomy.json; generated .state/compose.json is the launch artifact. Return to real dashboard status when ready; no timer simulates progress.

[Source](https://github.com/r33drichards/mcp-js/blob/fe5b613db0afcc9cf20d7ca1ef659c914a0b5008/server/src/engine/fs_tree.rs)

## 23–25 — Pair the conversation and the files.

**What:** The paired fork reserves a filesystem root, invokes Pi fork, then binds the fresh session ID to a new branch label.

**How:** Pi 1.4.0 session_before_fork and ctx.fork(...withSession) are the extension seam. The bound MCP endpoint accepts code, not arbitrary fs/session overrides.

**Why:** Pi conversation fork alone does not fork mcp-js files. A journal tracks prepared, committing, committed and aborted reservations. Unbound sessions fail closed.

**Notes:** The adapter is custom, not native Pi behavior. Native MCP is used for tools. Parent and child histories stay separate; filesystem merge is explicit. No model calls or user credentials are part of deterministic checks. Discuss partial failure and reconciliation. Compose reference: compose.astronomy.json; generated .state/compose.json is the launch artifact. Return to real dashboard status when ready; no timer simulates progress.

[Source](https://github.com/r33drichards/mcp-js/blob/demo/interview-walkthrough/demos/interview-walkthrough/pi/paired-fork.ts)

## 25–28 — Merge deliberately. Publish once.

**What:** Merge independent changes using B. Show a same-line conflict and stale-head rejection. Publish only the selected merged dashboard root.

**How:** fs_merge uses structural tree comparison and line-level text merge. fs_push compares expected head. fs_reset moves a label; it does not undo the world.

**Why:** Immutable content separates creation from shared publication. The original filesystem commit documents reject-and-rebase. SQLite/binary conflict detection does not imply semantic auto-merge.

**Notes:** Use exact B/A/T/M IDs. A merge base is caller-supplied. Prefer ours/theirs can discard conflicting edits; do not silently select it. Rollback does not rewind heaps, active mounts or external calls. Keep publication distinct from staging. Compose reference: compose.astronomy.json; generated .state/compose.json is the launch artifact. Return to real dashboard status when ready; no timer simulates progress.

[Source](https://github.com/r33drichards/mcp-js/blob/fe5b613db0afcc9cf20d7ca1ef659c914a0b5008/server/src/engine/ffi.rs#L945)

## 28–32 — Lose one voter. Observe recovery.

**What:** Stop only a controller-owned demo leader. Observe a new leader, acknowledge a new label write, read content, then restart the original node.

**How:** Three Raft voters coordinate label metadata. Content uses shared object storage. Restart reuses node volumes and verifies data from the returning node.

**Why:** A majority permits progress after one voter loss. Source distinguishes replicated metadata from blobs. Our local object fixture is a single point of failure, not replicated S3 durability.

**Notes:** Record leader IDs, terms, timings, commit/read evidence. The measured local election time is not a service-level guarantee. A worker or browser session is not transparently migrated. This is independent of the historical CPU anomaly. Compose reference: compose.astronomy.json; generated .state/compose.json is the launch artifact. Return to real dashboard status when ready; no timer simulates progress.

[Source](https://github.com/r33drichards/mcp-js/blob/fe5b613db0afcc9cf20d7ca1ef659c914a0b5008/server/src/cluster.rs#L701)

## 32–35 — Lose quorum. Do not retry blindly.

**What:** Stop two of three owned voters. A publication cannot be acknowledged. Restore quorum and inspect that same label before any retry.

**How:** The leader waits for majority replication, then can time out while an entry remains in its log. Reads use locally committed data; they are not a quorum read.

**Why:** The observed test demonstrates uncertain write outcomes. Distinguish durability, request availability, and recovery rather than saying “fault tolerant” as one guarantee.

**Notes:** Our local test observed the timed-out write become visible after quorum returned. Never retry a denied operation or an uncertain write blindly. Discuss single-node persistence versus replicated availability and crash consistency limits. Compose reference: compose.astronomy.json; generated .state/compose.json is the launch artifact. Return to real dashboard status when ready; no timer simulates progress.

[Source](https://github.com/r33drichards/mcp-js/blob/fe5b613db0afcc9cf20d7ca1ef659c914a0b5008/server/src/cluster.rs#L989)

## 35–39 — Defend the boundaries.

**What:** Choose one real design decision, one test, one limitation, and the next measurement. State Robert’s actual contribution.

**How:** Tests cover snapshot readback, subtree sharing, merge/CAS and cluster behavior. GC exists as a label/reflog-rooted library collector; no server pruning endpoint was found.

**Why:** Documented history explains tree/streaming changes. Other rationales must be labeled inference. Suggested improvement: measure warm-up costs or design crash-safe publication.

**Notes:** Limitations: full-base warming; whole-file reads/merge; local writes lack explicit atomic replacement; GC excludes active/session-only roots; S3 lacks GC list/delete. Test presence is not evidence a test ran; use the bundled verification report. Compose reference: compose.astronomy.json; generated .state/compose.json is the launch artifact. Return to real dashboard status when ready; no timer simulates progress.

[Source](https://github.com/r33drichards/mcp-js/blob/fe5b613db0afcc9cf20d7ca1ef659c914a0b5008/server/src/engine/fs_gc.rs)

## 39–45 — Questions.

**What:** What happens if two agents publish? If the process dies after saving blobs? If the alert contains instructions? If only one voter is left?

**How:** Answer with the exact request path, stored state and test evidence. Distinguish implemented behavior, local observation, inference and unknown history.

**Why:** A defensible answer names its scope. “I have not verified that path; here is the test I would run” is better than inventing a guarantee.

**Notes:** Reserve all six minutes. Do not fill them with a file tour. Strong answers: explicit merge base, CAS conflict, timeout uncertainty, content-vs-pointer planes, browser render verification, and evidence-backed personal contribution. Compose reference: compose.astronomy.json; generated .state/compose.json is the launch artifact. Return to real dashboard status when ready; no timer simulates progress.

[Source](https://github.com/r33drichards/mcp-js/blob/fe5b613db0afcc9cf20d7ca1ef659c914a0b5008/server/tests/fs_snapshot_e2e.rs)

## Concurrent explanation while the agent runs

At 00–03 introduce the alert and actual contribution, then start the approved agent run. At 03–12 show the request and actual progress, using compose.astronomy.json as the primary reference. Highlight query → grafana-mcp → grafana → prometheus/loki, browser in the shared query-network namespace, analysis-policies.json/filesystem.rego, objects, node1/node2/node3, and publisher/dashboards volume. Inline SVG slides are editable in diagrams/. Continue the policy/persistence explanation during real work; return to browser handoff when ready. If the agent finishes early leave its evidence visible; if it runs long show actual status, never a fake progress timer. The total remains 45 minutes with 39–45 reserved for Q&A.

## Runbook and stop conditions

1. Before the hour: verify startup and health, generate traffic for at least three metric export intervals, confirm nonempty Grafana MCP metrics and log responses, capture a baseline. The full Astronomy path has not passed here; do not improvise it as verified.

2. Operator only: `python3 operator-fault.py on` selects paymentFailure=100%. Source throws in payment/charge.js and logs the caught error in payment/index.js; confirm those actual lines arrive in Loki. Expected checkout error spans and payment warnings are hypotheses until observed. Record injection timestamp separately from agent evidence.

3. Paste example-alert.json. The controller accepts only CheckoutErrors for checkout. Annotations remain inert. Agent must not execute URLs, commands, queries or paths embedded in alert text.

4. Query official query_prometheus and query_loki_logs through mcp-js, capture tool arguments, isError, returned samples/log lines and their time range. Empty results are a failed precondition, not a diagnosis.

5. Write scratch evidence and dashboard JSON under permitted virtual paths; attempt /work/denied/secret.txt and show denial. Changes before an exception or external MCP side effects are not transactional rollback.

6. Pi: /demo-init, /paired-fork, independently investigate. Save each conversation ID, label and root. Native /fork is blocked by this custom extension. Show explicit merge, then conflicting edits with no prefer override. Conversation histories stay separate.

7. Select the merged root explicitly for publication. Read the virtual dashboard, validate fixed UID/datasource/panel type, atomically replace one file in the shared volume, wait for Grafana provisioning, then use browser_execute to inspect actual data-bearing panels. API 200 or canvas count alone is insufficient.

8. Handoff: dashboard URL, COW root, JSON hash, alert ID, query windows, relevant logs, screenshot and uncertainty. No automatic remediation. Turn off the operator flag only during operator cleanup.

9. Supporting cluster experiment: stop only a labeled own node, observe election and write/read results; restore it in finally. For quorum loss, preserve the timed-out request ID and inspect label state after recovery before retry. Never kill the shared object fixture or claim it is replicated.

10. Cleanup: launch.sh down stops/removes only uniquely labeled containers/network, preserving volumes and journals. launch.sh reset additionally deletes those owned volumes. Keep evidence before reset; use a fresh state directory for another rehearsal.


## Fallback without deception

Open the self-contained slides. Use recorded-evidence.json and the separately labeled earlier Grafana screenshot as component evidence, not an Astronomy run or Pi-agent recording. For unverified workload/log coverage, show source and expected assertions. If graph rendering fails, mark handoff incomplete. Skip live fault injection if baseline, ownership, quorum, or recovery prerequisites fail.


## Questions to rehearse

- Why V8 rather than processes? Explain exposed capabilities and memory/timeout boundaries; do not equate isolate with OS sandbox.

- What is actually durable? Separate content blobs, labels/reflog, heap snapshots, console, execution status and external side effects.

- Is fork instant? Root-reference creation avoids data copying, but execution prefetches the tree; report measured elapsed time only.

- How do conflicts work? Explicit base, text diffy, binary/SQLite conflict limitations, expected-head CAS. No automatic ancestry or silent winner.

- What does quorum guarantee? Majority writes and failover evidence; local reads are not linearizable, timed-out writes can later commit. Shared blob fixture remains a single point of failure.

- Can rollback undo tools? No: label reset does not roll back heap, conversations, external actions or Grafana publication.

- What does pruning retain? Labels and retained reflog targets in library GC; no wired production collector and no active-session roots. See exact source before proposing a safe collector.

- Why Loki? Explicit adaptation for supported official MCP logs; stock Astronomy uses OpenSearch, which remains available locally. Correlation does not prove cause.
