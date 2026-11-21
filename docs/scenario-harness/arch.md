# Scenario harness architecture (in detail)

Companion to README.md (explainer) and the three SVGs. This document goes
component-by-component with file paths, struct names, exact flows, and the
guarantees each layer provides. Everything below refers to the
`feat/scenario-real-sled` branch (PR #336, stack #312 -> #322 -> #336).

## 1. The full stack

```mermaid
flowchart TB
    subgraph SRC["Scenario sources"]
        FIX["Checked-in fixtures<br/>scenario_js.rs tests"]
        FUZZ["libFuzzer target<br/>fuzz_js_scenario.rs"]
    end
    subgraph RUNNER["Scenario runner<br/>engine-cluster-tests/src/scenario.rs"]
        SCHEMA["Scenario (serde)<br/>seed / topology / workload / faults / recovery / asserts"]
        DUMP["run_and_dump()<br/>scenario.json + result.json + event log"]
    end
    subgraph TRACKS["Two oracle tracks, one runner"]
        MODELED["Modeled runner<br/>scenario.rs run_scenario (no real-sled)<br/>in-memory ClusterNodes, handwritten hooks kept"]
        REAL["Real-I/O runner<br/>scenario_real.rs (feature real-sled)<br/>Turmoil hosts, real sled DBs"]
    end
    subgraph BOUNDARY["Storage boundary<br/>experiments/sled-fs (patched vendored sled)"]
        TSLED["sled + turmoil-fs<br/>per-host persistent /host-N"]
        FAULTS["real ENOSPC (filler writes)<br/>EIO at fs layer"]
    end
    FIX --> SCHEMA
    FUZZ --> SCHEMA
    SCHEMA --> DUMP
    SCHEMA --> MODELED
    SCHEMA --> REAL
    MODELED -.->|"in-memory faults only"| FAULTS
    REAL --> TSLED
    TSLED --> FAULTS
    MODELED --> DUMP
    REAL --> DUMP
```

The runner is the single entry point: `Scenario::run_and_dump(name)`
calls `run_scenario`, which under `feature = "real-sled"` delegates to
`scenario_real::run_scenario`, otherwise to the modeled runner. Both return
a `ScenarioResult` (status, acked / attempted / unknown / failed heads,
events) and both drive the same assertion list.

## 2. Scenario schema and semantics

`engine-cluster-tests/src/scenario.rs`:

- `Scenario { seed, topology, workload, faults, recovery, asserts }`
- `Topology { voters: Vec<String>, clients: Vec<String> }` - bounded; the
  real runner fixes voters to n0/n1/n2 and one observer client.
- `Workload { every_ms, count, op }`; `Op::RunJs { node, program }` where
  `node` resolves `"leader"` against the elected leader, or
  `Op::RunJsUnmodeled { source }` for arbitrary JS (separate oracle: JS
  exceptions and engine timeouts are first-class outcomes, not failures).
- `Program(Vec<Step>)`; `Step::Write { path, bytes }` with `{i}`
  iteration substitution; `render_js(i)` emits the JS that writes the file,
  snapshots, and publishes the shared label CAS.
- `Fault::Crash { at_ms, node: CrashTarget }` (`Node(name)` or
  `CurrentLeader`), `Fault::Partition { groups }` (groups may name
  `"current_leader"`; the sitting leader is excluded from any group that
  does not contain it), `Fault::HealAll`.
- `Recovery { stop_workload, heal_all, restart_crashed, deadline_ms }`.
- Assertion ids: `FreshWriteSucceeds`, `AckedCommitsInLegalHistory`,
  `ReplicasConvergeAfterRecovery`, `PublishedSnapshotMatchesModel`.

## 3. Real-I/O runner: control flow

```mermaid
flowchart TB
    subgraph CTRL["Controller (sim thread, scenario_real.rs)"]
        PEND["Control.pending: VecDeque (seq, HostAction)"]
        LOOP["step loop, budget 200k:<br/>pop pending -> Sim::crash / Sim::bounce<br/>then sim.step()"]
        RES["take_result() when observer finished"]
    end
    subgraph HOSTS["Turmoil hosts n0, n1, n2"]
        H["sled::Config::new().path(/host-N)<br/>open() per incarnation<br/>incarnation counter = generation"]
        NODE["ClusterNode::new(cfg, db)<br/>start_background_tasks()"]
        SH["ScenarioHost::new_with_generation(node, db, generation)"]
        LIST["TcpListener PORT (raft) + HOST_PORT (host RPC)<br/>JoinSet select loop"]
    end
    subgraph OBS["Observer client (run_client)"]
        LEAD["poll status() -> leader election event"]
        SCHED["interleave sorted faults and ops:<br/>tokio::select! biased<br/>(fault deadline vs publication future)"]
        PUB["publish(): run_js -> Head -> parse_ca_hex<br/>-> resolve(main) pre-read -> cas(main, expect, ca)"]
        REC["recovery: heal_all pairs,<br/>restart_crashed -> control_host(Bounce)"]
    end
    H --> NODE --> SH --> LIST
    PEND --> LOOP
    LOOP -->|"HostAction applied"| HOSTS
    HOSTS -->|"control_host: wait done >= seq"| PEND
    LEAD --> SCHED --> PUB
    SCHED --> REC
    PUB -->|"Outcome::Acked / Conflict / Unknown / Failed"| RES
```

Key points:

- **Controller-only faults.** Hosts cannot crash themselves or each other.
  A host that needs a fault applies `control_host(&control, HostAction)`,
  pushes `(seq, action)` and awaits `control.done >= seq`; the controller
  loop executes `sim.crash`/`sim.bounce` and acknowledges by sequence
  number. Partition/heal pairs are applied directly by the observer via
  `turmoil::partition`/`repair` (network faults, not process faults).
- **Leader resolution.** `CrashTarget::CurrentLeader` and
  `Partition["current_leader"]` resolve by polling
  `ScenarioHostClient::status()` across the voters at fault time.
- **Publication vs fault race.** While an op is in flight, the observer
  runs `tokio::select!` biased between the next fault deadline and the
  publication future - so a scheduled crash lands mid-publication rather
  than silently after it (verified by
  `scheduled_crash_interrupts_in_flight_js_publication`).
- **Outcome classes.** `Acked` (CAS moved the label),
  `Conflict` (label already at this head or CAS lost), `Unknown` (CAS
  errored - outcome genuinely uncertain), `Failed` (JS exception, timeout,
  bad head). A timeout is never interpreted as a non-commit; unknown
  outcomes are reconciled by re-reading `resolve(main)` before counting.

## 4. Publication, crash, and reopen

```mermaid
sequenceDiagram
    participant O as Observer
    participant H as ScenarioHost (gen 1)
    participant N as ClusterNode
    participant D as sled::Db (/host-0)
    O->>H: run_js(program.render_js(i))
    H->>N: engine executes JS in V8
    N->>D: put /data/iteration-i (durable)
    N->>D: snapshot blob + manifest
    H-->>O: JsResult::Head(ca)
    O->>H: resolve("main")
    H-->>O: current label (expect)
    O->>H: cas("main", expect, ca)
    H->>N: raft append + commit
    N->>D: durable append (label log)
    H-->>O: Ok(true) -> Acked
    Note over D: controller: Sim::crash(n0)
    Note over H,D: host task torn down, Db dropped,<br/>channels close
    Note over D: controller: Sim::bounce(n0)
    Note over D: exclusive fresh reopen,<br/>incarnation = gen + 1
    O->>H: status() -> (Leader, term), generation()
    O->>H: run_js(fresh program)
    H->>N: write + publish with new generation
    H-->>O: JsResult::Head(new ca) -> FreshWriteSucceeds
```

Guarantees exercised here:

- A pre-crash acknowledged commit stays in the legal Raft history
  (`AckedCommitsInLegalHistory`); `FreshWriteSucceeds` demands a *distinct*
  acknowledged post-recovery head and rejects prior/same-head
  acknowledgments, retrying uncertain outcomes.
- The reopened host must be *ready* before the restart event is recorded
  (`FreshReopenGenerationReady` before `Restart` in the event log).
- Two crashes must produce two distinct acknowledged snapshots
  (`owner_two_crashes_keep_distinct_acknowledged_js_snapshots`, gens 1->2->3).

## 5. Generation fencing (storage ownership)

```mermaid
stateDiagram-v2
    [*] --> Gen1
    Gen1 --> Crashed1 : Sim crash - Db dropped, channels close
    Crashed1 --> Gen2 : Sim bounce - exclusive fresh reopen
    Gen2 --> Crashed2 : Sim crash
    Crashed2 --> Gen3 : Sim bounce
    Gen1 --> Fenced : stale handle write
    Gen2 --> Fenced : stale handle write
    Fenced --> ErrClosed : write fails closed with error
    Gen3 --> [*]
```

Ownership rules, each enforced by a named regression:

| Rule | Enforcing test |
| --- | --- |
| Only the host runtime touches sled; workers get channel clones only | `blocking_worker_blobs_are_flushed_by_host_and_reopen` |
| Dropping the host aborts its blob pump | `dropping_scenario_host_stops_its_blob_pump` |
| Pre-crash completed console output is durable across reopen | `stale_console_writer_is_fenced_after_fresh_reopen` |
| A pre-crash captured writer cannot write post-reopen | `stale_console_writer_is_fenced_after_fresh_reopen` (writer is fenced: `Err`, not silently lost) |
| Cached handles fail closed after crash | same suite + `execution_registry_reuses_host_owned_persistent_sled` |
| Observer/hosts never exchange handles, only serialized RPC | `observer_runs_js_and_publishes_label_on_host`, `host_owned_engine_runs_js_on_patched_sled` |
| Malformed snapshot heads are errors, not absent files | `invalid_snapshot_head_is_not_reported_as_absent_file` |

Under the hood (server/src/engine): `HostBlobStorage` is an mpsc bridge
(`channel(16)` + pump future) over `SledHeapStorage` (blobs in the
`fs_blobs` tree, flushed on put/delete); `HostConsoleBridge` is an
unbounded channel with an `AtomicBool alive` revocation flag;
`write_page` fails with "console owner generation revoked" when revoked and
"console owner stopped" when the owning host is gone. `ConsoleLogState`
buffers per V8 thread (`RefCell`, single-threaded) and flushes WAL-style
pages by size.

## 6. Storage boundary proofs

The vendored sled in `experiments/sled-fs/sled` is patched for Turmoil's
filesystem (0.7.2, `unstable-fs`): strict directory model, no temporary
paths, faults surface through the real sled code paths. Named proofs:

- `storage.rs`: `enospc_reaches_real_sled_log_flush`,
  `eio_reaches_real_sled_config_write`,
  `crash_discards_pending_blob_bytes_written_by_sled`,
  `crash_recovery_without_destructor_flush`,
  `rejects_background_flusher_and_relative_paths`,
  `roundtrip_blobs_and_host_isolation`.
- `cluster.rs`: disk-full Raft node - append rejected while full, real
  ENOSPC from `db.flush()`, node stops ("cluster stopped or storage
  failed"), recoverable on reopen when space returns.
- `multi_node.rs`: `real_three_node_quorum_survives_disk_full_and_crash_reopen`
  - a real filler write exhausts a 32-KiB simulated disk on a live
  three-node cluster; quorum keeps committing and the reopened host catches up.
- `recovery.rs`: `disk_full_crash_reopen_preserves_ack_and_allows_fresh_progress`.

## 7. Artifacts, replay, minimization

```mermaid
flowchart LR
    RUN["run_and_dump(name)"] --> ART["artifacts dir:<br/>scenario.json (seed + exact faults)<br/>result.json (status, heads, full event log)"]
    ART --> REPLAY["deterministic replay:<br/>cargo test --manifest-path experiments/sled-fs/Cargo.toml --locked --features scenario --test scenario_js <test> -- --nocapture"]
    REPLAY --> MIN{"assertion still fails?"}
    MIN -->|"yes"| MIN2["minimize: drop one fault at a time,<br/>shrink topology to minimum voters,<br/>shrink JS program"]
    MIN2 --> MIN
    MIN -->|"no"| LAST["restore last failing case"]
    LAST --> REG["copy scenario into a #[test]<br/>with the same assertion -> commit as regression"]
```

Artifacts are the contract: a failure must replay without fuzzing, and a
minimized scenario is checked in as the regression (see README.md for the
exact commands).

## 8. CI and verification matrix

`.github/workflows/sled-fs-proof.yml` (toolchain 1.100,
`RUSTFLAGS=--cap-lints warn`) runs, in order:

1. `cargo test --features cluster` - real three-node Raft over turmoil TCP.
2. `cargo test --features scenario --test scenario_js` - the 13 real-sled
   scenario regressions (single-threaded).
3. `cargo test` (default features) - storage proofs.
4. `cargo test -p sled --features testing --lib --test test_log --test test_tree`
   - patched vendored sled's own suites.

Measured on `feat/scenario-real-sled`: scenario_js 13/13, cluster suite
green, storage/multi_node/recovery green, sled 66/66, server lib 299/299,
turmoil_engine (real-sled) 1/1. Milestone-6 hook audit and results:
`experiments/sled-fs/VALIDATION.md`.
