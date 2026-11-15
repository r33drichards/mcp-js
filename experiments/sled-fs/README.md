# Real sled filesystem fault proof

This standalone workspace patches **real sled 0.34.7**, not ClusterNode's storage
interface or its `fail_flush` flag. `cluster` compiles the existing
`server/src/cluster.rs` via `scheduler-raft-audit` and supplies a patched `sled::Db`.
No production manifest, dependency default, existing scenario, or cluster source
is changed. Do not enable this experiment in production.

## Proven slice

`src/fs.rs` in the vendored sled is a compile-time filesystem boundary. The
ordinary build re-exports `std::fs` and preserves Linux `sync_file_range`, its
ENOSYS fallback, fs2 locking, and normal worker/flusher behavior. The opt-in
`turmoil-fs` build re-exports **Turmoil =0.7.2**'s synchronous `unstable-fs` shim.
The existing Unix positional I/O loops work with its real `FileExt` implementation.
This is a module boundary rather than a public object-safe filesystem trait:
changing all public/config types to generic or boxed handles is deferred.

Simulation requires Unix, an absolute persistent path, `flush_every_ms(None)`,
one live opener per host/path, and all access and clean disposal within the
owning Turmoil host. Unsupported automatic flusher/relative/temporary configs
are rejected. `io_uring` and non-Unix simulation fail at compile time.

The tests exercise:

- Real sled config/log creation, insert, flush, reads, clean drop, reopen,
  large blobs, metadata, and host-local namespaces at the same absolute path.
  Round trips repeat seeds 7, 42, and 314.
- Fault-off success versus fault-on failure for Turmoil I/O errors during sled
  config writes, and disk exhaustion during sled log flushes (seed 314).
- Actual MCP-JS `ClusterNode::handle_append_entries`: first append succeeds;
  filling Turmoil's disk makes a later append fail and subsequent proposals
  report a stopped node. The healthy-disk control acknowledges both appends.
  No simulated-storage short circuit is set.
- A committed small value surviving crash, an unflushed value disappearing,
  and the flush-before-crash control surviving, across three seeds.
- Real sled-written blob bytes visible before crash, absent afterward, and
  sled recovery collecting that orphaned blob. This tests pending filesystem
  state, not just a value retained in sled's memory.
- A negative namespace control: a freshly created, flushed database loses its
  contents on crash without explicit directory synchronization in the fixture.
  That is a measured limitation, not a passing durability guarantee.

## Run

Rust/Cargo and a C compiler are needed. This was tested on x86_64 Linux with
Rust 1.100.0. Sled's old `deny(unused_qualifications)` triggers on that compiler;
cap dependency lints at warnings to run unchanged old code. No warning is
suppressed by deleting code or tests.

From the repository root:

```sh
export RUSTFLAGS="${RUSTFLAGS:+$RUSTFLAGS }--cap-lints warn"
cargo test --manifest-path experiments/sled-fs/Cargo.toml --locked
cargo test --manifest-path experiments/sled-fs/Cargo.toml --locked --features cluster -- --test-threads=1
cargo test --manifest-path experiments/sled-fs/Cargo.toml --locked -p sled --features testing --lib --test test_log --test test_tree -- --test-threads=1
cargo test --manifest-path server/cluster-tests/Cargo.toml --locked -- --test-threads=1
```

The first command tests the std backend, including OS lock exclusion and large
value recovery. `sim` alone runs filesystem tests without compiling the cluster
harness; `cluster` implies `sim`. The local `[patch.crates-io]` applies only to
this isolated workspace. Its lockfile pins Turmoil and records the full closure.

## Crash and determinism limits

Turmoil 0.7.2 `Sim::crash` calls `rt.crash()` **before** discarding pending FS
operations. That drops futures, and sled `Log::drop` flushes. The crash tests
therefore explicitly `mem::forget` the bounded Db instance before yielding.
There are no background flushers/workers that can later persist it. The old
instance is never used again; a fresh Db performs recovery. This leaks a small
amount of memory per test; it is a test technique, not a production crash API.
Do not use normal Db drop as a crash. A reusable crash-safe ownership mechanism
and invalidation of outstanding handles remain future work.

Turmoil `FsHandle::enter` (`src/fs/mod.rs:139-145`) constructs a worker context
with `Mutex::new(rand::rng())` and capture-time `time`. OS worker interleavings,
RNG, and time would therefore escape seeded simulation. This patch does not use
FsHandle. Under `turmoil-fs`, sled uses its existing inline threadpool alternative;
the real periodic flusher is forbidden by config validation. This changes
scheduling and may expose reentrancy/deadlock risks in larger workloads. Passing
these bounded seeded tests is **not** a claim of deterministic concurrent sled,
or coverage of production thread scheduling. Do not combine `testing` or
`lock_free_delays` with simulation to infer replay guarantees.

There is no Turmoil equivalent for fs2 OS locks. Simulation intentionally omits
that call and assumes a single opener per host/path; duplicate openers and
cross-process exclusion are unmodeled (not enforced). Shim File operations use
the current host context, so Db handles must not migrate between hosts or OS
threads. Those restrictions need an owned host/generation-bound file wrapper
before general cluster lifecycle integration.

Linux log writeback is modeled by whole-file `sync_all` in simulation, which is
stronger than the original `sync_file_range` behavior. Torn writes within that
write/sync pair cannot be scheduled by this synchronous slice. Turmoil supports
`block_size`, `sync_probability`, `corruption_probability`, and capacity/I/O
faults, but this proof does not cover torn-write/corruption campaigns or injected
fsync failure. Its I/O error and space-exhaustion APIs return `ErrorKind::Other`
with text, **not** raw errno 5/28; tests check propagated error messages. `set_len`
does not check capacity in Turmoil 0.7.2; the cluster test fills the disk using
actual writes.

Sled does not sync config creation, blob contents, or parent directory entries
in the reviewed routes. The positive small-value recovery fixture explicitly
syncs initial config and directories, then relies on sled's log sync. The negative
control leaves that setup unsynced and confirms data loss. Blob roundtrip tests
are clean-reopen tests, not a promise that flushed blob values survive crashes.
A complete durability study must separately cover new databases, blob references,
snapshot rename/delete ordering, namespace loss, and interrupted writes.

## Source provenance and review

The branch starts at MCP-JS PR #314 head
`5a3065def361f13b745378119c3e53184f98232a`, based on #312 head
`b0a7f0aca741977a4028c1611d0eb3cc3b764bdd`, verified via GitHub metadata.
MCP-JS's lockfile resolves sled to 0.34.7. `sled/` is the crates.io 0.34.7
release with its licenses and original tests preserved. The release's
`.cargo_vcs_info.json` identifies upstream commit
`d81865d07f07910133877915b57abf0c52d5756b` ("Cut 0.34.7"); a separate upstream
clone at that commit had identical `src/` to the published crate.

`adapter.patch` is the small reviewable delta against the published crate,
including one existing test compilation repair (`let mut sub` for
`Subscriber::next_timeout(&mut self)`). It does not disable any upstream test.
See [SOURCE_MAP.md](SOURCE_MAP.md) for all filesystem and scheduling routes and
[VALIDATION.md](VALIDATION.md) for exact results and checks not run.
