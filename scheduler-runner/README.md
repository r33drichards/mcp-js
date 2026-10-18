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
stores** and refuses automatic migration of existing clusters. Nonempty unsealed
stores (including a local scheduler_state tree), SQLite prototypes and unrelated
nonempty directories are rejected without resetting their contents. Only learner
membership may change after the voter map is sealed. Keep cluster ports on the
existing trusted internal network: Raft RPC authentication is not implemented.

Send newline-delimited JSON to the current leader's stdin. This is an operator
interface, **not an MCP transport or a multi-tenant authorization boundary**:

```json
{"action":"create","definition":{"name":"example","code":"console.log('scheduled')","cron":"* * * * *","timezone":"UTC","policy":{"missed":"Skip","overlap":"Skip","retry":{"max_retries":0,"delay_seconds":0}}}}
{"action":"status"}
{"action":"list"}
{"action":"disable","id":1}
{"action":"delete","id":1}
{"action":"acknowledge_stopped","occurrence_id":2,"dispatcher_and_work_quiesced":true}
```

Status works on every node and reports its current role, leader ID and addresses;
use it to choose the leader's terminal. List explicitly returns a stale local view.
Mutations require committed quorum acknowledgements. Acknowledge stopped requires
an explicit operator attestation that **the old dispatcher can no longer submit,
all in-flight submissions are resolved, and external execution has ended**. Merely
seeing no running worker execution is insufficient: a delayed submission could
still arrive. This assertion is not a technical proof. It releases an Unknown
overlap/capacity reservation. EOF closes
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

## Reproduce the tested topology

The self-contained binary integration test starts **three actual runner processes**
with unique IDs, dynamically allocated loopback ports and separate fresh sled
stores. Every process knows the other two fixed voters. A fourth loopback HTTP
process acts as the worker. The test sends stdin status/create/list, waits for a
real minute-cron dispatch, kills the active leader after the worker accepts it,
then verifies Unknown on takeover, disable/delete and exactly one automatic POST.
The mock deliberately keeps execution running; no real credential is created.

```sh
cargo test --manifest-path scheduler-runner/Cargo.toml --locked --test binary -- --nocapture
```

This test takes up to approximately 75 seconds because it exercises a real minute
boundary. Deterministic clock/restart/overlap tests are in the other suites.

For an existing authenticated worker, generate three local configurations below.
Set `WORKER_ORIGIN` to that worker's HTTPS origin and make the **existing**
`MCP_SCHEDULER_WORKER_TOKEN` available in each runner terminal. The generator never
reads or stores its value. It writes empty initial schedules and a one-hour grant.

```sh
cargo build --manifest-path scheduler-runner/Cargo.toml --locked
export RUNNER_DEMO_DIR="$(mktemp -d /tmp/mcp-runner-demo.XXXXXX)"
python3 - <<'PYCONFIG'
import datetime, json, os, pathlib
root = pathlib.Path(os.environ["RUNNER_DEMO_DIR"])
expiry = (datetime.datetime.now(datetime.timezone.utc) + datetime.timedelta(hours=1)).isoformat()
remote = {"url": os.environ["WORKER_ORIGIN"], "bearer_env": "MCP_SCHEDULER_WORKER_TOKEN",
          "expires_at": expiry, "session": "operator-scheduler", "timeout_seconds": 30, "heap_mb": 128}
for i in range(1, 4):
    config = {"node_id": f"runner-{i}", "advertise_addr": f"127.0.0.1:{4400+i}",
              "cluster_port": 4400+i, "peers": [f"runner-{j}@127.0.0.1:{4400+j}" for j in range(1, 4) if j != i],
              "data_dir": str(root / f"data-{i}"), "schedules": [], "remote": remote}
    (root / f"runner-{i}.json").write_text(json.dumps(config))
print(root)
PYCONFIG
```

In three terminals, substitute the same printed directory for `DEMO_DIR`:

```sh
scheduler-runner/target/debug/mcp-js-scheduler-runner DEMO_DIR/runner-1.json
scheduler-runner/target/debug/mcp-js-scheduler-runner DEMO_DIR/runner-2.json
scheduler-runner/target/debug/mcp-js-scheduler-runner DEMO_DIR/runner-3.json
```

Each line above runs in its own terminal; enter the status JSON there, then the
management examples on the leader. If `CARGO_TARGET_DIR` is set, use its `debug/`
binary path instead. Do not reuse these directories to change voter membership.

## Safety scope requiring a decision

The current invariant is **at most one automatic application-level submission
per occurrence attempt by these non-Byzantine runners**. Stale owners cannot claim
new work or commit completion receipts. A previously consumed request may still
start or continue after takeover. This is not worker-start fencing.

If the required contract is instead “reject an old owner's request at worker
admission,” the worker API must participate: carry occurrence/attempt identity and
owner epoch, then atomically validate and consume/deduplicate the permit through
consensus. A runner-only check immediately before POST cannot close that race.
Preventing effects after admission additionally needs cancellation or fencing at
the affected resource. Those semantics are an architectural decision, not a
capability claimed by this prototype. MCP management would likewise need the
authenticated principal-to-schedule/grant binding left open by the design; the
implemented usable management path is explicitly local operator stdin.
