# tokio-cron-scheduler alternative

This branch uses `tokio-cron-scheduler = 0.15.1`. `PulseSource` installs one async
cron job in `JobScheduler` with a one-second schedule. The callback sends a
coalescing observation pulse and returns. Default in-memory stores are used;
PostgreSQL and NATS features are not enabled. A supervisor invokes native shutdown
when the source stops or is dropped. Payload execution and draining remain in
the common runner, outside the native job callback.

The [pinned scheduler source](https://docs.rs/tokio-cron-scheduler/0.15.1/src/tokio_cron_scheduler/scheduler.rs.html)
checks due metadata, activates jobs, and advances cron timestamps from the current
observation. That does not establish durable application receipts or a configurable
missed-slot replay contract. The [constructor and lifecycle source](https://docs.rs/tokio-cron-scheduler/0.15.1/src/tokio_cron_scheduler/job_scheduler.rs.html)
shows the default store setup and shutdown path. Our user-facing cron expressions
are parsed by the common Croner 4.0.1 layer, not this driver's internal parser.

The library provides cron/one-shot/repeated jobs, async callbacks, job lifecycle
notifications, and optional persistence adapters. None of those is used here as
proof of execution completion, no-overlap, recovery replay, or external-effect
deduplication. The core's sled receipts and executor outcomes supply those
application decisions. See [tokio-cron-scheduler 0.15.1](https://docs.rs/tokio-cron-scheduler/0.15.1/tokio_cron_scheduler/).

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
