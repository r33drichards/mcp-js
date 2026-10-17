# Apalis runner-only Raft integration

Experimental, operator-owned standalone runner. It compiles the repository's
existing `server/src/cluster.rs` directly and has **no Engine, Deno or V8 dependency**.
Apalis supplies timer pulses. The shared application core decides missed runs,
overlap, retries and retention; the existing sled-backed Raft implementation
commits the authoritative checkpoint. No consensus library replacement is added.

This extends only PR284. PR285 remains the local tokio-cron comparison. The common
policy acceptance suite remains unchanged; this distributed adapter is not a
claim that Apalis itself supplies distributed persistence or failover.

## Run and manage

```sh
cargo build --manifest-path scheduler-runner/Cargo.toml --locked
cargo run --manifest-path scheduler-runner/Cargo.toml --locked -- scheduler-runner/example.json
```

Adapt the example to an existing worker and an existing credential reference;
it contains no credential and creates no schedules. The operator supplies the
referenced environment variable outside the replicated store. HTTPS is required
except for loopback testing. Redirects and HTTP client retries are disabled.
Worker requests retain the ordinary bearer-authentication path and specify
execution/heap limits. The session field is a logging tag, not authenticated identity.
The worker must enforce its normal authentication and execution policies.

For three voters, give each a unique node ID, address, port and data directory,
and list the other two as `id@host:port` peers. All voters must use the same
identities/addresses and hardened code. This prototype requires **fresh consensus
stores** and refuses automatic migration of existing clusters. Only learner
membership may change after the voter map is sealed. Keep cluster ports on the
existing trusted internal network: Raft RPC authentication is not implemented.

Send newline-delimited JSON to the current leader's stdin. This is an operator
interface, **not an MCP transport or a multi-tenant authorization boundary**:

```json
{"action":"create","definition":{"name":"example","code":"console.log('scheduled')","cron":"* * * * *","timezone":"UTC","policy":{"missed":"Skip","overlap":"Skip","retry":{"max_retries":0,"delay_seconds":0}}}}
{"action":"list"}
{"action":"disable","id":1}
{"action":"delete","id":1}
{"action":"acknowledge_stopped","occurrence_id":2}
```

List explicitly returns a stale local view. Mutations require committed quorum
acknowledgements. Acknowledge stopped only after independently establishing that
execution ended; it releases an Unknown overlap/capacity reservation. EOF closes
management input but leaves scheduling active. Ctrl-C stops admission and drains
remote observation futures (bounded by their observation deadline). No operation
cancels arbitrary remote effects.

Initial schedules, limits and the grant reference/expiry are bound in the
replicated execution contract. They must match on every replica and restart.
Changing that contract requires an explicit future migration; it is not silently
adopted. Local create/delete changes remain in Raft across restart. Token content
is never stored in snapshots; expired grants stop new claims. Revocation at the
worker remains authoritative. An uncertain management result may have committed;
inspect state before repeating creation (no creation-request deduplication yet).

## Ownership and execution boundary

1. A leader commits a current-term barrier, bringing any inherited prefix into
   the applied state before loading its checkpoint.
2. Acquisition CAS commits a new revision/owner/term, converting old Running
   receipts to Unknown. Expected term is checked under the proposal append lock.
3. Each change runs in a disposable local planner. Only a successful exact-revision
   Raft CAS exposes its result. Cancellation or error invalidates the planner.
4. A claim is durably consumed before the single HTTP POST. Neither failover nor
   restart replays it. Missing quorum means no invocation is released.
5. HTTP 202 is acceptance only. The adapter polls the returned execution ID.
   Completed means success; confirmed failed permits the configured bounded retry.
   Lost/malformed acknowledgements, absent status, observation deadline, cancellation
   and timeout become Unknown, retaining overlap/capacity reservations.

Generic cluster put/CAS operations reject the scheduler namespace, including
companion writes. This does not protect against malicious Raft peers: the cluster
transport still assumes trusted peers and network isolation.

A crash between consume and send can lose work. A stale runner may still send an
already consumed request after leadership changes; takeover preserves it as Unknown
and never submits it again. There is **no worker-side fencing/idempotency endpoint**,
atomic transaction with external effects, lossless replay or exactly-once guarantee.
The conservative protocol trades availability for avoiding automatic duplicate
submission. Confirmed-failure retries can repeat partial external effects.

## Validation and limits

Run:

```sh
cargo test --manifest-path scheduler-runner/Cargo.toml --locked
cargo clippy --manifest-path scheduler-runner/Cargo.toml --locked --all-targets -- -D warnings
cargo test --manifest-path server/cluster-tests/Cargo.toml --locked -- --test-threads=1
cargo test --manifest-path scheduler-core/Cargo.toml --locked
```

Tests cover fixed-voter three-node failover/quorum loss, stale-owner rejection,
node reconstruction, abrupt subprocess exit after durable consumption, namespace
protection, accepted-versus-completed HTTP responses, lost/malformed ACKs, terminal
status classification and expired grants. The shared 33-test core suite covers
policy combinations and four subprocess crash boundaries. Mock HTTP tests exercise
the real worker API shapes; they do not certify a deployed worker's authentication
or external effects. Process exit is not a power-loss durability test.

This is not production ready. It has no MCP management/authentication binding,
Events delivery, existing-store migration, worker-side execution permits, throughput
benchmark, Raft snapshotting/log compaction, or automatic resolution of Unknown
work. Full checkpoints are written to an unbounded consensus log; synchronous sled
flushes and temporary planner creation suit small experiments, not sustained load.
No production service or security settings are changed.
