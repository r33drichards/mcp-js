# I/O boundary source map

All paths below are relative to `sled/src/`. They refer to the 0.34.7 source,
not sled main. `adapter.patch` records exact replacements.

| Route | Original operations | Experimental treatment / coverage |
| --- | --- | --- |
| `config.rs`, `Config::open_file`, `write_config`, `read_config` | Directory existence, mkdir, OpenOptions, create_new, read/write/seek/metadata, config serialization | Route through `crate::fs`. Config writes now fsync and newly created blob/journal paths plus ancestor directories sync under the opt-in simulation feature; crash recovery without fixture sync is tested. Std behavior unchanged. Absolute paths required in simulation to avoid cwd dependence. |
| `config.rs`, `try_lock` | fs2 blocking/test or nonblocking/production exclusive lock | Std unchanged; explicitly unmodeled under simulation, single opener required. |
| `config.rs`, `RunningConfig::get_snapshot_files`, Config drop | Path exists/read_dir and remove_dir_all | Path methods replaced with boundary calls. Temporary simulation config rejected; no simulated temp cleanup promise. |
| `pagecache/parallel_io_unix.rs` | FileExt read_at/read_exact_at/write_all_at, short/interrupted I/O loops | Uses boundary File; real std Unix FileExt is implemented by Turmoil. Log flush and recovery exercised. |
| `pagecache/parallel_io_windows.rs`, `parallel_io_polyfill.rs` | Windows seek_read/seek_write, fallback clone/seek/read/write | File imports routed; ordinary backend preserved. Non-Unix simulation deliberately compile-rejected. Cross-compilation not tested. |
| `pagecache/iobuf.rs`, `write_to_log` | Positional writes, raw fd Linux sync_file_range; non-Linux fsync; optional rio | `fs::sync_range` preserves default algorithm. Simulation uses whole-file fsync. io_uring plus simulation rejected. Disk-full flush tested. |
| `pagecache/logger.rs` | File type and destructor flush/sync | File import routed. Clean reopen tested; crash tests prevent destructor execution. |
| `pagecache/blob_io.rs` | Open/create_new/read/write, enumerate blobs, remove orphan and obsolete files | All through boundary. Large-value roundtrip plus pending-blob loss and orphan removal tested. Blob crash durability not guaranteed. |
| `pagecache/snapshot.rs` | Open/read/seek, mkdir, write/fsync, rename and removal; recovery tail writes and file sync | All FS calls routed. Recovery tested, but no complete snapshot rename/delete fault matrix. |
| `pagecache/mod.rs`, size_on_disk | Log metadata, blob read_dir/metadata | Routed and tested with large blobs. |
| `pagecache/segment.rs` | set_len/sync_all on config.file, truncation in threadpool | Boundary file propagates calls; worker scheduling becomes inline. No exhaustive truncation fault test. |
| `flusher.rs` | OS thread, periodic roll/GC/file sync, shutdown join | Ordinary backend unchanged. Validation rejects periodic flusher in simulation. |
| `threadpool.rs` and `lib.rs` inline alternative | Shared process queue and dynamic std threads | Simulation selects existing inline implementation. Covers calls from logger/iobuf, recovery iterator, segment truncation, Tree::flush_async. The async flush API and concurrency stress are not simulated here. |
| `pagecache/iobuf.rs`, `iterator.rs`, `logger.rs`, `segment.rs`, `tree.rs` | Worker task submissions | All use crate threadpool, so simulation calls remain inside host context. Inline execution is a scheduling change, not a full deterministic executor. |
| `sys_limits.rs` | Reads host `/sys/fs/cgroup/memory/memory.limit_in_bytes`; libc getrlimit/sysconf | Intentionally remains real OS environment introspection, not database storage. Tests set explicit small cache_capacity; full constructor hermeticity is not claimed. |
| docs, unit/integration tests | Example std::fs cleanup and test subprocesses | Preserved; not application I/O. Existing upstream tests exercise the std backend. |

Turmoil 0.7.2 sources inspected:

- `src/fs/shim/std/fs/mod.rs`: files, directory entries, sequential read/write/seek,
  metadata, rename/remove, file/directory synchronization, error injection.
- `src/fs/shim/std/os/unix/fs/mod.rs`: actual std FileExt implementation.
- `src/fs/mod.rs`: FsConfig, host FS state, pending/durable operations,
  discard_pending/torn writes, FsHandle::current/enter and worker RNG.
- `src/sim.rs`: crash runtime teardown precedes filesystem discard; bounce.
- `tests/fs/durability.rs` and `tests/fs/threads.rs`: version-matched examples.

## Work before a complete abstraction

1. Give each file/backend an owning host identity and crash generation; prevent
   cross-host/thread use and stale handle activity. Implement modeled exclusive
   locks and test duplicate-open exclusion without changing std locks.
2. Replace inline work with a deterministic executor compatible with sled's
   synchronization, or instrument blocking work with scheduler-owned RNG/time.
   Capturing FsHandle alone is insufficient.
3. Add an explicit abrupt-stop mechanism that cannot run sled shutdown writes;
   remove deliberate test leaks and coordinate outstanding work.
4. Model Linux writeback separately from durable fsync; expose write/sync
   interruption points and error injection at each boundary.
5. Test snapshot, blob, new-directory, truncation, and teardown failures under
   realistic ordering. The adapter explicitly syncs initial config and names;
   distinguish this from upstream sled behavior. Blob/snapshot sync gaps remain.
6. Extend the cluster lifecycle harness to use the backend safely with multiple
   nodes, worker tasks and restarts. This proof only adds a direct real-cluster
   disk-full test; it does not alter the independent scenario implementation.
