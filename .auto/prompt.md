# Optimize the Rust filesystem implementation

The objective is lower cold S3 end-to-end time, summed across both full
datasets. Initialize `s3_cold_wall_ms` (unit ms, direction lower). First measure and log the unchanged checkout as the
baseline. Record warm S3, local disk, and guest scan timings. Repeat
promising measurements when the change is close to baseline variability.

Run `./.auto/measure.sh` through run_experiment with timeout_seconds 21600
and checks_timeout_seconds 1800. It builds the candidate Rust server and
runs the unchanged PR benchmark on all 14,754 MDN Markdown files and all
385 Wikipedia shards, split as in GitHub Actions, with two passes per
backend and the 20/60 ms median/p95 log-normal S3 simulation. The full
corpora are pinned and downloaded once into a shared cache. This machine
runs two slices at a time. The GitHub benchmark uses more runners and
its timings are a separate comparison. Check exact full-corpus counts
and pass the Rust filesystem regression tests before retaining changes.

Read `.auto/references.md` and the source checkouts. Start with hypotheses
around bounded concurrent snapshot prefetch, deduplicating warm requests,
avoiding repeated manifest and metadata lookups, and reducing cache-path
or Rust filesystem operation overhead. Measure each hypothesis separately
and record costs in disk I/O, concurrency and memory where applicable.
The Rust scope is the filesystem path in `server/src/engine`, primarily
`fs_store.rs`, `fs_mount.rs`, `fs.rs`, `heap_storage.rs`, and the mount
integration in `mod.rs`. Focus shared storage changes on filesystem
behavior and preserve other callers' semantics.

Keep the benchmark JavaScript, inputs, timing boundaries, simulation,
and experiment scaffolding fixed. Preserve snapshot compatibility,
policy checks, errors, and content-address correctness. Keep candidates
local to the research branch. The default cap is 33 measurements, including
one baseline and up to 32 follow-up runs. Report verified winners and
regressions at the cap.
