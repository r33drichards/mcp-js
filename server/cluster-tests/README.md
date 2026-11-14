# Consensus regression harness

This standalone Cargo workspace compiles the **actual** `server/src/cluster.rs`
without linking V8. It runs the preserved scheduler claim reproducers, focused
consensus regressions and the existing networked cluster/learner tests:

```sh
cargo test --manifest-path server/cluster-tests/Cargo.toml --locked -- --test-threads=1
```

The `turmoil_network` integration test drives the real cluster HTTP handler over
simulated TCP. It checks a healthy request, refusal during a network partition,
and recovery after repair. Peer-to-peer Raft traffic still uses reqwest and is
not simulated by this test.

The `turmoil_fuzz` integration test explores 32 deterministic seeds by default,
with 24 randomized network faults and HTTP requests per seed: partitions,
one-way partitions, repairs, latency/loss, malformed RPCs, votes, and appends.
It checks term and index bounds, keeps an already-committed receipt immutable,
checks complete HTTP responses, and surfaces handler panics. A failed assertion
prints its seed and step. Replay a single seed or expand the bounded search:

```sh
TURMOIL_FUZZ_SEED=123 cargo test --manifest-path server/cluster-tests/Cargo.toml --locked --test turmoil_fuzz
TURMOIL_FUZZ_CASES=1000 cargo test --manifest-path server/cluster-tests/Cargo.toml --locked --test turmoil_fuzz
```

This is replayable, seeded fault-sequence fuzzing, not coverage-guided libFuzzer
or full Raft network simulation: outbound peer RPCs still use reqwest.

Before this patch both preserved reproducers failed: a proposal replaced by a
new leader returned `Ok(true)`, and a rejected mismatched-prefix heartbeat
truncated an already committed receipt. These are regression tests, not a formal
proof of the consensus implementation.

Successful votes and replication acknowledgements now require sled flushes.
Committed state-machine writes and the applied-index marker share one atomic
batch, followed by a flush. Storage failures stop the node and make outstanding
client operations fail with an uncertain outcome. A client success checks the
original proposal, leadership term and durably applied index. A CAS mismatch
against pending state is provisional, not a linearizable read.

`ClusterNode::new_fixed` seals a fresh store to an explicit voter map. Every voter
must be configured with the same identities and addresses and run the hardened
implementation. Existing stores are rejected rather than migrated implicitly.
The seal survives restart through either constructor. Learners may join/leave;
voters may not change. Legacy dynamic voting membership remains unsupported for
scheduler fencing: it has no joint-consensus configuration protocol.

The cluster RPC/data interface remains a trusted internal-network interface.
This patch adds no authentication, production configuration or credentials.
Local reads are stale and cannot authorize execution. Any integration must
obtain a successful current-term committed operation before dispatch. Consensus
and consume-before-send receipts do not make external effects exactly once.
