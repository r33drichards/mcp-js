# Live interview walkthrough: alert to dashboard in 45 minutes

Audience: interviewers evaluating Robert’s work (confirmed). The challenge brief remains unknown. This is a live engineering discussion with slides and demonstrations, not a narrated video. Fill in Robert’s actual contribution, constraints and decision history before rehearsal; suggested explanations are not claims about his motivations.

Implementation reference: [fe5b613db0afcc9cf20d7ca1ef659c914a0b5008](https://github.com/r33drichards/mcp-js/commit/fe5b613db0afcc9cf20d7ca1ef659c914a0b5008). Read [architecture-reference.md](architecture-reference.md) for exact symbols, disk representation and history, and [verification.md](verification.md) for executed checks and remaining gaps. The prior COW reference is this same inspected base; no assumption about later engine changes is implied.

## Prepare the room, not a video

Open four views: [slides.html](slides.html), the approved Pi terminal, the actual generated Compose configuration, and Grafana. Use arrow keys to navigate, N for notes, A for the agenda. The visible live cue names the next screen, observable proof and transition. Optional silent illustrations are collapsed and loaded only when explicitly opened; skip them in the main run.

Before the clock starts:

1. Follow [README startup and Pi instructions](README.md). Confirm health, browser reachability, nonempty checkout metrics and payment/checkout logs. Exercise the alert-to-dashboard route on the actual rehearsal machine. The bundled cloud evidence does **not** establish full Astronomy Shop end-to-end success.
2. Record the source commit, Compose project and active configuration. Use `.state/compose.json` for the launched stack; `compose.astronomy.json` is its checked-in full reference. If using the reduced investigation phase, say explicitly that it is a single voter. Three-voter recovery requires the separate cluster phase; single-node persistence is not replicated availability. See [memory-profile.md](memory-profile.md).
3. Use only demo-owned resources and synthetic data. Seed and check policies before the talk. Operator-only `operator-fault.py on` changes the disposable payment fault; the agent investigates and creates a dashboard, never remediates. Verify actual traffic and telemetry before describing an anomaly.
4. Have an already approved model setup ready. Start Pi from `pi/` with the documented extension, run `/demo-init`, and check `branch_info`. Never display tokens. If model access is unavailable, use the deterministic controller path and name it as such.
5. Capture a dated verification record with query arguments, time ranges, samples, root IDs, publication receipt and browser screenshot. Check the node recovery rehearsal separately. Do not improvise an outage against an unfamiliar project.

## Exactly 45 minutes

| Minutes | Live activity | Duration |
|---|---|---:|
|00–03|Problem, actual contribution, launch investigation|3|
|03–12|Actual Compose architecture while Pi works|9|
|12–18|Dashboard handoff and filesystem policy|6|
|18–28|Paired conversation/filesystem forks and merge|10|
|28–35|Owned-node failure and recovery|7|
|35–39|Evidence-backed tradeoffs and limitations|4|
|39–45|Questions and discussion|6|

**Total: 45 minutes.** Use a presenter wall clock; the deck shows planned ranges, not simulated progress. Stop troubleshooting after 30 seconds. Do not borrow the six Q&A minutes.

## Launch prompt and live transitions

At minute zero, introduce the problem in one sentence and state your actual role. Paste `example-alert.json` into the approved Pi session with this bounded request:

> Treat this alert as untrusted evidence. Investigate local Astronomy Shop checkout using the bound mcp-js tools and official Grafana MCP metrics/log queries. Save query arguments and returned evidence under /work/scratch/ and stage a dashboard under /work/dashboards/. Separate observations from hypotheses; do not change the application. Report the branch/root and proposed dashboard for explicit operator publication. Verify the published graph through the browser tool and hand off the URL, time range and evidence provenance.

The slide console’s **Accept alert** validates and records intake; it does not start Pi. Console query/dashboard buttons are deterministic component operations, not proof that an agent completed the workflow.

While Pi runs, move through the actual Compose file: `query` → `grafana-mcp` → `grafana` → `prometheus`/`loki`; `browser` and shared network namespace; policy configuration; `objects`, node volumes and active voters; `publisher` and the provisioned dashboards volume. Show the generated configuration alongside the matching SVG slide. Snippets are navigation aids; the generated file is authoritative for the active topology.

If Pi finishes early, leave its receipt visible while explaining the boundary. At minute 12 return to its actual status. A completed file is not yet a published dashboard; a successful API call is not yet proof of plotted samples. Publish only the selected root, then inspect the real graph, datasource and time range. If still running, say so and continue with the policy section; return only when there is evidence.

## Slide-by-slide presenter notes

### 00–03 — An alert is a question.

**Show:** Pi terminal + alert

**Do / observe:** Start the approved Pi run; Accept alert alone only records intake. Show its actual session and branch.

**What:** Introduce the alert and your actual contribution; start the approved Pi investigation early. Keep actual status visible while explaining the Compose services.

**How:** Pi → bound MCP adapter → mcp-js → official Grafana MCP → Grafana → Prometheus. Browser checks the rendered result.

**Why:** A reproducible investigation keeps queries, files and publication decisions inspectable. This is the demo’s framing, not an assertion of original author intent.

**Speaker evidence:** Open with your actual role and the requirement you owned. The brief is still unknown. Pin the implementation to fe5b613. Operator-injected application faults and cluster outages are separate experiments. Code defaults heap_store/fs_store to none despite README default-state wording.

**Transition:** While the investigation runs, follow its request through Compose.

**Discussion prompt:** Which part did you personally implement, and what requirement drove it?

[Implementation source](https://github.com/r33drichards/mcp-js/blob/fe5b613db0afcc9cf20d7ca1ef659c914a0b5008/server/src/cli.rs#L196)

### 03–06 — Read the alert.

**Show:** Generated Compose + Pi status

**Do / observe:** Show query, grafana-mcp, grafana, prometheus and loki in the active generated file.

**What:** Accept CheckoutErrors for checkout. Reject an out-of-scope service. Retain annotations as untrusted evidence.

**How:** The adapter validates a small JSON envelope. It chooses fixed local resources; alert annotations do not become commands, paths or PromQL.

**Why:** A narrow demo scope makes investigation reproducible and avoids turning an alert body into executable authority. Inference: reduce capability surface.

**Speaker evidence:** Use the alert panel. Astronomy Shop 2.1.3 is pinned. Stock OTLP logs fan out to added Loki for the official Grafana MCP log tool. Full workload execution is not yet verified. The demo investigates, it does not repair anything. During generation open compose.astronomy.json. Follow query, grafana-mcp, browser, node1/node2/node3, objects and publisher. Do not fake a timed completion.

**Transition:** Next inspect the tool result, rather than infer success from a running process.

**Discussion prompt:** What prevents alert text from becoming authority?

[Implementation source](https://github.com/r33drichards/mcp-js/blob/demo/interview-walkthrough/demos/interview-walkthrough/alerts.py)

### 06–09 — Ask the metrics.

**Show:** Tool result + Compose telemetry services

**Do / observe:** Show query_prometheus arguments, nonempty timestamped samples and query_loki_logs results.

**What:** Inspect checkout request rate by status, latency and payment/checkout logs. Save actual tool responses under /work/scratch/.

**How:** mcp.callTool("grafana", "query_prometheus", …) uses the official server and provisioned demo-prometheus datasource. The query result goes into a COW snapshot.

**Why:** The route preserves datasource configuration and observable tool results. No invented tool names and no direct Prometheus shortcut in the agent query.

**Speaker evidence:** Show the exact tool schema and arguments. Read isError, terminal execution status and returned data. Pi model calls were not made; deterministic integration checks use the same MCP path. Never imply deterministic checks are an agent recording.

**Transition:** Trace where these responses are stored and which state survives execution.

**Discussion prompt:** What evidence supports a hypothesis, and what would disprove it?

[Implementation source](https://github.com/r33drichards/mcp-js/blob/fe5b613db0afcc9cf20d7ca1ef659c914a0b5008/server/src/engine/mcp_client.rs#L830)

### 09–12 — Execution ends. State is explicit.

**Show:** Compose query, browser, policies, objects and volumes

**Do / observe:** Show the mounted policy files, COW storage configuration and actual active node topology.

**What:** Follow submission, execution UUID, console output, fs root and session history. Identify which pieces survive.

**How:** Tokio task → V8 semaphore → blocking worker → deno_core isolate → snapshot flush. Heap and fs persistence are independent. Execution status is in memory; console output is in sled.

**Why:** Isolation and persistence solve separate problems. Source implements separate state axes; this allows fs persistence with a fresh heap. Tradeoff: no transaction over all external effects.

**Speaker evidence:** Timeout requests V8 termination; it is not a whole-request hard deadline. Warming/queueing precede the timeout selection. OS sandbox is optional process-wide defense. Cite the print-alias fix as a concrete security lesson without claiming authorship.

**Transition:** Return to the dashboard if ready; otherwise state its real status.

**Discussion prompt:** What survives a restart, and what is only in memory?

[Implementation source](https://github.com/r33drichards/mcp-js/blob/fe5b613db0afcc9cf20d7ca1ef659c914a0b5008/server/src/engine/mod.rs#L2458)

### 12–15 — Make the handoff visible.

**Show:** Grafana browser + publication receipt

**Do / observe:** Show selected root, file hash, datasource, time range and actual plotted samples.

**What:** Stage dashboard JSON in /work/dashboards/. Explicitly export one selected root. Verify the browser sees time-series panels with data.

**How:** A separate publisher materializes the virtual file, validates its identity, writes a temporary host file, fsyncs and renames it. Grafana polls the shared provisioned directory.

**Why:** COW storage is not a host mount. The explicit boundary prevents two investigation branches from racing writes into the live dashboard.

**Speaker evidence:** Show the root, source path, SHA-256, Grafana URL, and browser evidence. Provisioning API success alone does not establish visible graphs. If the browser is unavailable, label the handoff incomplete rather than showing an illustrative graph as live.

**Transition:** Now test the boundary that constrained those writes.

**Discussion prompt:** Why is API success insufficient evidence of a usable handoff?

[Implementation source](https://github.com/r33drichards/mcp-js/blob/fe5b613db0afcc9cf20d7ca1ef659c914a0b5008/server/src/engine/fs_mount.rs#L337)

### 15–18 — Allow one path. Deny another.

**Show:** Allowed / denied action + filesystem.rego

**Do / observe:** Compare successful scratch write with explicit protected-path policy denial; read back unchanged content.

**What:** Read/write /work/scratch/ and /work/dashboards/. A write to /work/protected/ fails. Existing snapshot remains readable.

**How:** FsService gates effective operation/path before dispatch to the overlay. Demo Rego checks operation plus namespace and rejects traversal components.

**Why:** Per-operation policy provides a boundary more precise than handing the guest the host filesystem. Source comments document this gate; Robert supplies the original decision history.

**Speaker evidence:** Policy input is operation, not op. Show failed status/error rather than treating any exception as proof of policy denial. No passthrough. Directory creation is implicit in virtual mode. Additional hardening is opt-in.

**Transition:** Use that same virtual tree as the common base for two investigations.

**Discussion prompt:** Would a host volume provide this branch isolation?

[Implementation source](https://github.com/r33drichards/mcp-js/blob/fe5b613db0afcc9cf20d7ca1ef659c914a0b5008/server/src/engine/fs.rs#L143)

### 18–23 — Fork two views of the same bytes.

**Show:** Pi sessions + branch roots

**Do / observe:** Fork two investigations from B; write distinct findings and read B again. Record root IDs.

**What:** Both investigations start at B. A and T change their own files. Read B again to prove it is unchanged.

**How:** SessionMount pins base_id and records Data/Whiteout entries. Push rewrites changed tree paths; chunks and unchanged subtrees are shared.

**Why:** Commit 4d6e446 documents recursive trees, lazy traversal and copy-by-reference. Dedup is by content. There is no filesystem commit ancestry in a root.

**Speaker evidence:** Bincode BTreeMap directory nodes use BLAKE3; <=64KiB files inline; larger files use FastCDC 256KiB/1MiB/4MiB with optional zstd. Caveat: execution prefetch traverses the base, so no unmeasured “instant fork” or O(touched) startup claim.

**Transition:** Explain the custom session binding that keeps each investigation on its own branch.

**Discussion prompt:** What is copied, what is shared, and what work is still proportional to data?

[Implementation source](https://github.com/r33drichards/mcp-js/blob/fe5b613db0afcc9cf20d7ca1ef659c914a0b5008/server/src/engine/fs_tree.rs)

### 23–25 — Pair the conversation and the files.

**Show:** Pi /paired-fork + binding journal

**Do / observe:** Show branch_info for both sessions. Demonstrate the custom pairing only if rehearsed; otherwise show its test record.

**What:** The paired fork reserves a filesystem root, invokes Pi fork, then binds the fresh session ID to a new branch label.

**How:** Pi 1.4.0 session_before_fork and ctx.fork(...withSession) are the extension seam. The bound MCP endpoint accepts code, not arbitrary fs/session overrides.

**Why:** Pi conversation fork alone does not fork mcp-js files. A journal tracks prepared, committing, committed and aborted reservations. Unbound sessions fail closed.

**Speaker evidence:** The adapter is custom, not native Pi behavior. Native MCP is used for tools. Parent and child histories stay separate; filesystem merge is explicit. No model calls or user credentials are part of deterministic checks. Discuss partial failure and reconciliation.

**Transition:** Merge the filesystem findings explicitly; keep the conversation histories separate.

**Discussion prompt:** What happens if the session fork succeeds but binding fails?

[Implementation source](https://github.com/r33drichards/mcp-js/blob/demo/interview-walkthrough/demos/interview-walkthrough/pi/paired-fork.ts)

### 25–28 — Merge deliberately. Publish once.

**Show:** Merge + CAS, then Conflict check

**Do / observe:** Use explicit B/A/T roots; show merged result, conflict and stale-head rejection before selected-root publication.

**What:** Merge independent changes using B. Show a same-line conflict and stale-head rejection. Publish only the selected merged dashboard root.

**How:** fs_merge uses structural tree comparison and line-level text merge. fs_push compares expected head. fs_reset moves a label; it does not undo the world.

**Why:** Immutable content separates creation from shared publication. The original filesystem commit documents reject-and-rebase. SQLite/binary conflict detection does not imply semantic auto-merge.

**Speaker evidence:** Use exact B/A/T/M IDs. A merge base is caller-supplied. Prefer ours/theirs can discard conflicting edits; do not silently select it. Rollback does not rewind heaps, active mounts or external calls. Keep publication distinct from staging.

**Transition:** Freeze the dashboard evidence before the separate infrastructure experiment.

**Discussion prompt:** What does reset undo, and what can it never undo?

[Implementation source](https://github.com/r33drichards/mcp-js/blob/fe5b613db0afcc9cf20d7ca1ef659c914a0b5008/server/src/engine/ffi.rs#L945)

### 28–32 — Lose one voter. Observe recovery.

**Show:** Three-voter status + owned leader experiment

**Do / observe:** Confirm three healthy voters first; record leader, acknowledged write and readback after restart.

**What:** Stop only a controller-owned demo leader. Observe a new leader, acknowledge a new label write, read content, then restart the original node.

**How:** Three Raft voters coordinate label metadata. Content uses shared object storage. Restart reuses node volumes and verifies data from the returning node.

**Why:** A majority permits progress after one voter loss. Source distinguishes replicated metadata from blobs. Our local object fixture is a single point of failure, not replicated S3 durability.

**Speaker evidence:** Record leader IDs, terms, timings, commit/read evidence. The measured local election time is not a service-level guarantee. A worker or browser session is not transparently migrated. This is independent of the Astronomy Shop application fault.

**Transition:** Separate successful recovery from what happens without a majority.

**Discussion prompt:** Which failure domain remains unreplicated?

[Implementation source](https://github.com/r33drichards/mcp-js/blob/fe5b613db0afcc9cf20d7ca1ef659c914a0b5008/server/src/cluster.rs#L701)

### 32–35 — Lose quorum. Do not retry blindly.

**Show:** Quorum evidence + same-label readback

**Do / observe:** Use the rehearsed quorum experiment only with time remaining; inspect uncertain outcomes before retry.

**What:** Stop two of three owned voters. A publication cannot be acknowledged. Restore quorum and inspect that same label before any retry.

**How:** The leader waits for majority replication, then can time out while an entry remains in its log. Reads use locally committed data; they are not a quorum read.

**Why:** The observed test demonstrates uncertain write outcomes. Distinguish durability, request availability, and recovery rather than saying “fault tolerant” as one guarantee.

**Speaker evidence:** Our local test observed the timed-out write become visible after quorum returned. Never retry a denied operation or an uncertain write blindly. Discuss single-node persistence versus replicated availability and crash consistency limits.

**Transition:** Restore all owned nodes and move to tradeoffs by minute 35.

**Discussion prompt:** Can a timed-out write later become visible?

[Implementation source](https://github.com/r33drichards/mcp-js/blob/fe5b613db0afcc9cf20d7ca1ef659c914a0b5008/server/src/cluster.rs#L989)

### 35–39 — Defend the boundaries.

**Show:** Source, test report and one decision

**Do / observe:** Choose one documented tradeoff, one observed test and one unresolved measurement.

**What:** Choose one real design decision, one test, one limitation, and the next measurement. State Robert’s actual contribution.

**How:** Tests cover snapshot readback, subtree sharing, merge/CAS and cluster behavior. GC exists as a label/reflog-rooted library collector; no server pruning endpoint was found.

**Why:** Documented history explains tree/streaming changes. Other rationales must be labeled inference. Suggested improvement: measure warm-up costs or design crash-safe publication.

**Speaker evidence:** Limitations: full-base warming; whole-file reads/merge; local writes lack explicit atomic replacement; GC excludes active/session-only roots; S3 lacks GC list/delete. Test presence is not evidence a test ran; use the bundled verification report.

**Transition:** Stop presenting at minute 39 and invite questions.

**Discussion prompt:** What alternative did you consider? Supply your actual decision history.

[Implementation source](https://github.com/r33drichards/mcp-js/blob/fe5b613db0afcc9cf20d7ca1ef659c914a0b5008/server/src/engine/fs_gc.rs)

### 39–45 — Questions.

**Show:** Questions + evidence on demand

**Do / observe:** Reserve all six minutes. Open source or a test result to answer a specific challenge.

**What:** What happens if two agents publish? If the process dies after saving blobs? If the alert contains instructions? If only one voter is left?

**How:** Answer with the exact request path, stored state and test evidence. Distinguish implemented behavior, local observation, inference and unknown history.

**Why:** A defensible answer names its scope. “I have not verified that path; here is the test I would run” is better than inventing a guarantee.

**Speaker evidence:** Reserve all six minutes. Do not fill them with a file tour. Strong answers: explicit merge base, CAS conflict, timeout uncertainty, content-vs-pointer planes, browser render verification, and evidence-backed personal contribution.

**Transition:** End at minute 45 with the dashboard link and evidence provenance.

**Discussion prompt:** Which invariant would you like to challenge?

[Implementation source](https://github.com/r33drichards/mcp-js/blob/fe5b613db0afcc9cf20d7ca1ef659c914a0b5008/server/tests/fs_snapshot_e2e.rs)

## Recovery and fallback rules

- **Slow agent or missing credentials:** show the actual status; switch explicitly to deterministic controller checks. Do not label this a model run. Keep investigation outputs separate from earlier test fixtures.
- **Empty metrics/logs:** show the empty response and time range. Do not assert a root cause or fabricate graph samples. Explain the missing evidence and use a clearly dated, provenance-labeled capture if available.
- **Browser failure:** retain the publication receipt, but call the visual handoff incomplete. Prior synthetic-fixture browser evidence is not an Astronomy recording.
- **Fork/merge problem:** preserve root IDs and the reservation journal; do not bind both sessions to the parent. Show deterministic evidence if the interactive extension has not been rehearsed. Never silently resolve a conflict with ours/theirs.
- **Cluster experiment:** require three healthy owned voters and restorable volumes. Stop on unexpected ownership/topology, an already unhealthy cluster, or an uncertain result. Restore stopped owned nodes and inspect the same label before retrying a timed-out write. If recovery overruns, skip the second outage and use the existing labeled quorum test evidence.
- **Static fallback:** disconnect the controller and select Recorded checks. Show exact source, test record and limitations; SVGs and silent animations illustrate mechanisms and do not prove execution.

Do not reset or delete useful state during the interview. Afterward follow the owned-resource cleanup in README, preserving evidence and volumes until no longer needed. AWS operations and shutdown belong to the separately authorized deployment task.

## Likely questions: short defensible answers

| Question | Evidence-backed answer |
|---|---|
| Is fork instant? | Roots share immutable content, but execution prefetch traverses the base. Measure latency; do not promise constant-time startup. |
| Why not a shared host directory? | The overlay isolates branch edits. Publication explicitly materializes one selected root to Grafana; it is a separate external effect. |
| What survives failure? | Separate immutable blobs, mutable label metadata, persisted console data, heap configuration and in-memory execution status. None is an all-effects transaction. |
| Can writes conflict? | Merge uses a caller-supplied base; text merges and binary conflicts differ. Expected-head CAS prevents stale publication. |
| Can reset undo the dashboard? | Reset moves a label. It does not rewind external Grafana publication, heaps or active mounts. |
| Are three nodes highly available? | Majority metadata writes can progress after one voter loss. Local reads can lag; shared object storage here is a single failure point. |
| Is a timed-out write safe to retry? | No assumption: the quorum test observed later commitment. Inspect the same target first. |
| How is storage pruned? | A label/reflog-rooted GC library exists; no server pruning endpoint was found. Active/session-only roots and S3 listing/deletion are limitations. |
| Why was this design chosen? | Cite documented history where available, label deductions as inference, and let Robert supply his actual decisions and alternatives. |

At minute 45 hand off the actual dashboard link plus receipt and provenance, or state precisely which handoff check remains incomplete.
