# Raft conformance test options

Research baseline: checkout `2a8a88a4` (2026-10-27); researched November 15, 2026.

## Recommendation

Extend `server/cluster-tests`, which already compiles the real cluster module
without V8. Start with deterministic protocol cases derived from Raft Figure 2
and etcd's tests, then add seeded network-fault histories and a Porcupine checker.
There is no implementation-independent Raft certification suite that can simply
be pointed at this HTTP API. Passing these tests increases confidence; it is not
a proof of correctness.

SQLite metadata support is independent: `SqliteStore` is not a replacement for
`ClusterNode`'s sled persistence. A future SQLite Raft persistence adapter needs
both storage crash tests and the same consensus tests run against it. rqlite
would delegate consensus instead and require adapter/transaction/consistency
contract tests, rather than testing this app's protocol implementation.

## Existing coverage and boundaries

- `server/src/cluster_regressions.rs`: committed-prefix preservation, stale
  replies, malformed appends, crash/reopen of applied state, failed flushes,
  atomic companions, fixed-voter membership, learner restrictions, higher terms,
  slow-voter quorum liveness, and cancellation of old replication work.
- `server/tests/cluster_simulation.rs`: five localhost nodes with temporary sled
  databases, leader/follower crash and restart. Despite the filename, this uses
  real Tokio sockets and timing, not a deterministic simulated network.
- `server/tests/cluster_learner.rs`: learner elections, replication and quorum.
- `server/cluster-tests/src/lib.rs`: a replaced scheduler claim must not be
  acknowledged; an inconsistent prefix must not truncate committed state.
- `.github/workflows/cluster-consensus.yml`: standalone harness in CI, without V8.
- `specs/`: existing formal models can complement executable tests, but a model
  passing does not establish that the Rust implementation refines it.

The checkout exposes test flush-failure injection but uses concrete
`reqwest::Client` for peer RPC and `tokio::net` for listeners. Injectable PeerRpc
and Turmoil work is visible on other remote history, not in this baseline;
review/reuse it before creating another transport seam.

Local `get` is intentionally stale. CAS mismatch can reflect pending proposals
and is explicitly not a linearizable ownership observation. Fixed-voter mode
rejects voter changes; legacy join/leave is not a joint-consensus protocol.
Conformance claims must state those limits.

## Candidate tools and suites

| Source | What it tests | Fit and adaptation cost |
| --- | --- | --- |
| [Raft paper, Figure 2 and Figure 8](https://raft.github.io/raft.pdf) | Protocol rules and the old-term commitment counterexample | Primary behavioral specification. Translate into table-driven Rust handler and quorum tests. Small initial cost. |
| [etcd raft tests](https://github.com/etcd-io/raft/blob/main/raft_test.go) | Elections, voting/log freshness, replication, current-term commitment, stale messages, learners | Best concrete case catalog. Go tests exercise etcd internals, so port scenarios/assertions, not the harness. PreVote, ReadIndex and membership tests apply only when those features exist here. |
| [raft-rs harness](https://github.com/tikv/raft-rs/tree/master/harness) | Rust protocol/network scenarios around raft-rs types | Useful Rust reference; not generic over arbitrary consensus engines. Adapting every internal message type would cost more than adding focused cases to our existing harness. |
| [OpenRaft testing utilities](https://docs.rs/openraft/0.9.21/openraft/testing/index.html) | Implementations of OpenRaft storage traits | Useful if adopting OpenRaft; these are not protocol conformance tests for `ClusterNode`. A rusqlite metadata adapter does not implement those traits. APIs vary by OpenRaft version. |
| [Turmoil](https://github.com/tokio-rs/turmoil) | Deterministic simulation, network partitions/delays, reproducible seeds | Strong fit after transport/timer/randomness are controlled. Adding the dependency alone does not simulate reqwest traffic or sled power-loss behavior. Pin a version and its supported I/O APIs. |
| [Porcupine](https://github.com/anishathalye/porcupine) | Linearizability of concurrent operation histories against a sequential model | Export JSON histories from Rust and run a small Go checker. Checks client-visible behavior, not every internal Raft invariant. Multi-key companion writes need a combined model; per-key partitioning would miss cross-key atomicity. |
| [Jepsen](https://github.com/jepsen-io/jepsen) | Real-process fault workloads and histories | Good later/nightly end-to-end coverage: kill/restart, network partitions, latency. Higher cost: deployment controller, workload, nemesis and checker. It is a framework, not a turnkey test for this server. |
| [Ongaro's Raft TLA+ spec](https://github.com/ongardie/raft.tla) | Model-level protocol safety | Reference for invariants and bounded scenarios. Needs an explicit implementation mapping or replay bridge; cannot execute Rust directly. |

When porting source code rather than independently translating documented
scenarios, retain the upstream license notices and attribution.

## Reviewed simulation PR stack

The user identified the active deterministic simulation work. Current PRs are
[#312](https://github.com/r33drichards/mcp-js/pull/312) (network/RPC/restart/fuzz),
[#314](https://github.com/r33drichards/mcp-js/pull/314) (filesystem fuzz model), and
[#317](https://github.com/r33drichards/mcp-js/pull/317) (real sled filesystem-fault
experiment), stacked in that order. A separate `test/raft-conformance` worktree
extends #317 with seeded vote/reopen, six-case freshness, duplicate/stale/conflict
append tests and the current-term commitment regression described below. This
keeps SQLite metadata changes independent of the simulation stack.

## First additions: protocol matrix in the existing harness

Use real `handle_request_vote`/`handle_append_entries` calls and real persisted
state. Prefer temporary directories and durable reopen over only mutating memory.
The existing regression tests overlap some cases; extend them instead of adding
duplicate names for the same behavior.

| Priority | Scenario | Required assertion |
| --- | --- | --- |
| P0 | Vote A, persist, drop all handles, reopen, receive B's request in the same term | B is rejected; a retry by A is allowed. At most one vote per term survives restart. |
| P0 | Candidate freshness matrix: shorter/higher-term, longer/lower-term, equal-term shorter/equal/longer logs | Freshness compares last term first, then index; a stale candidate never wins a vote. |
| P0 | Figure 8: new leader replicates an old-term entry to a majority | Counting replicas alone does not commit it. Committing a current-term entry subsequently commits its prefix. Adapt etcd `TestCannotCommitWithoutNewTermEntry`. |
| P0 | Partition old leader from a majority; elect and commit on the majority; heal | Minority cannot acknowledge a new commit. Previously acknowledged entries survive; conflicting uncommitted suffix is repaired. A node may temporarily still call itself leader: assert at most one elected leader **per term**, not one role label across all terms. |
| P0 | Duplicate and reorder appends and responses, including replies from an earlier leadership term | No duplicate state-machine effects, commit/applied indices never retreat, stale responses never manufacture quorum. |
| P0 | Crash at each durable transition: term/vote, append, application batch, applied marker, response | No acknowledgement before the required durability; recovered state preserves every acknowledged commit. Include uncertainty when persistence succeeds but the response is lost. |
| P1 | Identical prefix, divergent suffix, short heartbeat, absent prev index, incorrect prev term | Matching prefix retained; only uncommitted conflicts truncated; commit bounded by matched entries. Extend existing short-heartbeat and prefix tests. |
| P1 | Five voters, two unavailable or arbitrarily slow; stable reachable majority | Eventual election and write completion under bounded latency. With fewer than three reachable voters, no new successful commit. Bound fairness/latency assumptions explicitly. |
| P1 | Learner receives a log and sends responses during a voter partition | Learner cannot vote, lead, or supply the missing voter quorum. |
| P2 | Lagging node catches up through bounded batches and restart | Eventually applies the same committed prefix without duplicate companions. Snapshot installation only enters the matrix if implemented. |

Safety checks should run after every scheduled event: one elected leader per
term, matching log prefixes for equal index/term, immutable acknowledged entries,
monotonic commit/applied indices per node, and identical state-machine application
for the same index. Maintain a historical oracle; comparing only current node
states can miss a value that was acknowledged and later overwritten.

## History checking: specify the contract first

Record invocation, response, node identity, operation ID, key/value, companions,
and outcome (`success`, definite rejection, or unknown) on one harness clock.
A timeout is unknown: the operation may have committed. Keep its invocation
pending or model optional completion; do not silently remove it as a failure.
Likewise, a storage error after writing may have an uncertain outcome.

Initial checker scope should cover successful committed writes/CAS and durable
final observations after the workload stops and quorum catches up. This is a
restricted history check, not a claim that the entire API is linearizable.
Specifically, do not feed ordinary stale `get` or provisional `CAS=false` results
into a standard linearizable-register model and then weaken the checker to make
it pass. To claim whole-API linearizability, first add a quorum-confirmed read
barrier and a definitive committed-state CAS-failure response, then test those
operations too. Multi-key head/reflog operations must remain atomic in the model.

## Practical rollout

1. Add the vote/restart, freshness and Figure 8 cases to the V8-free harness and
   its current PR workflow. No new external test service is needed.
2. Review the remote PeerRpc/simulation work, then put election/append/forwarding
   RPCs through one injectable interface. Control election randomness and time.
   Reuse the production serialization/handler paths in simulations.
3. Run fixed seeds in PR CI for 3/5 voters, learner, asymmetric partition,
   duplicate/delayed replies, leader crash/reopen and healing. Save seed, event
   trace, operation history and node logs on failure; replay the trace exactly.
4. Add a pinned Porcupine checker and validate it with known-good and intentionally
   illegal histories, including an acknowledged write that disappears. Run it on
   generated histories; promote each discovered bug to a minimal Rust regression.
5. Run a broader seed budget and process-level Jepsen-style fault tests nightly.
   Test real filesystem failures separately: a flush error hook is not a model
   of torn writes, fsync ordering or full-machine power loss.

Success criteria: every test exercises production code, a deliberately broken
vote/commit/truncation implementation is detected, failures have replayable
artifacts, and CI keeps storage-contract tests separate from consensus tests.
