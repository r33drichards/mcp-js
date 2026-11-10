# Lazy runtime filesystem reads

`run_js` validates the pinned filesystem root at mount time. It fetches child
nodes and file chunks only when a filesystem operation needs them. The S3
backend's existing runtime bridge executes requests on the Tokio runtime that
owns its client, even when an async or synchronous guest operation runs on a
different runtime. No cache is required for correctness.

Previously, `build_fs_mount` called `SessionMount::warm`, walking all descendant
nodes and all chunk references before each execution. On uncached S3 this read
all previously stored contents again. With a bounded local cache, a snapshot
larger than the cache could evict warmed data before the execution started.
Repeated imports therefore paid for an ever-growing unrelated dataset.

## Cost and behavior

- Mount: one root node lookup, or a decoded-node cache hit. Root node size still
  depends on its number of immediate children.
- Metadata-only operations: requested directory nodes, no file chunk reads.
- Reads: requested paths and chunks, using the configured bounded cache.
- Writes: new chunks and changed tree paths. Unchanged subtree hashes and the
  existing snapshot/session commit flow are preserved.
- Engine-owned filesystem service tasks run independently of the isolate so
  overlapping async and synchronous guest calls cannot deadlock on the mount
  lock. Tasks are cancelled and drained before snapshot publication or terminal
  completion. The deno_core op driver stays on its current-thread runtime.
- Append and symlink mutations propagate storage failures rather than treating
  unreadable data or metadata as a nonexistent file.
- Invalid root IDs still fail before running JavaScript. Unavailable descendants
  or contents now fail when accessed instead of failing every unrelated run.
- `FsStore::prefetch` and `SessionMount::warm` remain available as explicit,
  potentially expensive cache hints, not a guarantee that all data stays local.

This does not make whole-file reads or appends streaming, increase transfer
request limits, change snapshot formats, or change the running importer.

## Regression and request-count benchmark

Run with the repository's pinned development environment:

```sh
nix develop --command cargo test --locked -p server --lib lazy_fs_ -- --test-threads=1 --nocapture
```

The tests use the production AWS S3 SDK against a disposable loopback HTTP
object fixture, then execute real `run_js` calls. No AWS account or live bucket
is contacted. They cover cold async and synchronous reads/writes, fresh decoded
node caches, uncached S3, a forced-eviction local cache, a warm working set,
metadata-only access, persistence after reopening, deferred read errors, gated
async/sync overlap, cancellation, and draining the actual mutable mount.
V8 cancellation does not itself wake an outstanding Rust network future; these
tests do not claim immediate cancellation of already-dispatched S3 requests.

The request-count benchmark compares explicit legacy full prefetch with a
complete lazy no-op execution on snapshots of 4 and 128 chunked files, each in
its own directory. Assertions require `2 * files + 1` GETs for full prefetch
versus one root GET for the complete lazy run. It prints observed local timing
only for diagnosis; these are not production AWS latency or throughput claims.

The `Runtime filesystem regression` workflow also runs the existing filesystem
persistence, policy, compatibility, cluster/cache, and large-file tests.

## Local validation (2026-11-10)

With Rust 1.99.1 and the checked-in Cargo.lock:

- Complete server library suite: 302 passed, including all six new regressions.
- Eight filesystem integration suites: 49 passed; the opt-in 1 GiB test was not run.
- Guest filesystem hook integration: five passed.
- New test fixture formatting, workflow YAML parsing, and diff whitespace checks passed.

The existing compiler deprecation/dead-code warnings remain. Nix packaging,
GitHub Actions, production AWS latency, and live deployment have not been
validated by these local runs.
