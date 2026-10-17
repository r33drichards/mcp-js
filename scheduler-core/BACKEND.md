# Apalis alternative

This branch uses `apalis = 1.0.0`, `apalis-cron = 1.0.0`, and `cron = 0.17.0`.
`PulseSource` builds an Apalis worker over `CronScheduler` with a one-second
schedule. Its handler sends a coalescing observation pulse and returns. Apalis
retry middleware is disabled and no persistent Apalis backend is installed.
Shutdown aborts and joins this pulse worker; payload executions belong to the
common runner and have a separate drain lifecycle.

The [pinned CronScheduler source](https://docs.rs/apalis-cron/1.0.0/src/apalis_cron/backend.rs.html)
classifies this backend as `Ephemeral`. It computes a next tick, waits, emits it,
and computes another. The [cron adapter](https://docs.rs/apalis-cron/1.0.0/src/apalis_cron/schedule/cron.rs.html)
uses upcoming wall-clock occurrences. An in-memory timer is not a durable
schedule/occurrence ledger. This implementation does not infer replay or missed-run
backfill from Apalis's optional storage support.

Apalis provides a worker/backend abstraction and optional middleware and durable
queue integrations. Those are potential future extensions, not guarantees of
this configuration. Adding a persistent queue or retry layer would require a new
review of the core's consume-before-submit and no-unknown-retry contracts.
See [Apalis 1.0 documentation](https://docs.rs/apalis/1.0.0/apalis/) and
[apalis-cron 1.0 examples](https://docs.rs/apalis-cron/1.0.0/apalis_cron/).

| Comparison | Apalis branch | tokio-cron branch |
|---|---|---|
| Native integration | Worker over a cron backend | One async job in `JobScheduler` |
| Native execution retry | No retry layer installed | No application retries delegated to driver |
| Native persistence in this configuration | Ephemeral cron backend | Default in-memory scheduler stores |
| Durable policy storage | Shared application sled snapshot | Same |
| Missed/overlap/retry decisions | Shared application core | Same |
| Future expansion | Worker middleware and queue backends | Job lifecycle and optional storage adapters |
| Tradeoff | Broader worker abstraction for a small timer role | Smaller conceptual timer interface; native scheduler has internal actors |

This is an implementation comparison, not a performance benchmark or a comparison
of native persistence backends. No exactly-once, lossless replay, or distributed
execution property is supplied or claimed by either adapter.

The later `scheduler-runner` extension uses the repository's hardened Raft path
for authoritative checkpoints. It is application integration, not an Apalis
persistent backend. PR285 does not include that extension. See the runner README
for scope, fresh fixed-voter requirements, consume-before-send losses and limitations.
