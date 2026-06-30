//! Benchmark: does wrapping a remote-primary blob backend with the existing
//! `WriteThroughCacheHeapStorage` actually speed up the fs snapshot store?
//!
//! Real S3 is unavailable in the test environment, so `LatencyStorage` wraps an
//! in-memory store with a per-op `tokio::time::sleep` to model a remote
//! round-trip. The benchmark then measures, with and without the write-through
//! cache in front of that latency primary:
//!
//!   1. write phase  — put_file / push a snapshot (PUTs go to primary + cache)
//!   2. cold read    — fresh store, pull + read every file back
//!   3. warm read    — same store, re-read (cache hits, no primary round-trip)
//!
//! Run:   cargo test --test fs_cache_bench -- --nocapture --ignored
//!        (marked #[ignore] because it is a multi-second measurement, not a
//!         correctness assertion.)

use server::engine::fs_mount::SessionMount;
use server::engine::fs_store::FsStore;
use server::engine::heap_storage::{
    HeapStorage, MemoryHeapStorage, WriteThroughCacheHeapStorage,
};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tempfile::TempDir;

/// Modeled network latency per blob round-trip (put or get). 5 ms is in the
/// ballpark of a cross-region S3 GET for a small object.
const LATENCY: Duration = Duration::from_millis(5);

/// A `HeapStorage` that delegates to an inner backend but sleeps LATENCY before
/// every `put`/`get`, simulating a remote primary. `list`/`contains`/`delete`
/// hit the inner store without artificial delay (they are not the hot path here).
#[derive(Clone)]
struct LatencyStorage {
    inner: MemoryHeapStorage,
}

#[async_trait::async_trait]
impl HeapStorage for LatencyStorage {
    async fn put(&self, name: &str, data: &[u8]) -> Result<(), String> {
        tokio::time::sleep(LATENCY).await;
        self.inner.put(name, data).await
    }
    async fn get(&self, name: &str) -> Result<Vec<u8>, String> {
        tokio::time::sleep(LATENCY).await;
        self.inner.get(name).await
    }
    async fn list(&self) -> Result<Vec<String>, String> {
        self.inner.list().await
    }
    async fn delete(&self, name: &str) -> Result<(), String> {
        self.inner.delete(name).await
    }
    async fn contains(&self, name: &str) -> Result<bool, String> {
        self.inner.contains(name).await
    }
}

/// Deterministic pseudo-random bytes so FastCDC finds real content boundaries.
fn pseudo_random(len: usize, seed: u64) -> Vec<u8> {
    let mut state = seed | 1;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 24) as u8
        })
        .collect()
}

struct Workload {
    /// Each (path, bytes). Mix of small (inlined) and large (chunked) files so
    /// both the tree-node and chunk-blob paths are exercised.
    files: Vec<(String, Vec<u8>)>,
}

fn workload() -> Workload {
    let mut files = Vec::new();
    // 200 small files (inlined, no chunks): stresses tree-node PUT/GET.
    for i in 0..200u32 {
        let p = format!("src/mod_{i}.txt");
        files.push((p, pseudo_random(2 * 1024, i as u64)));
    }
    // 4 large files (~2 MiB each => multiple 256 KiB..4 MiB chunks): stresses
    // chunk-blob PUT/GET and streamed chunking.
    for i in 0..4u32 {
        let p = format!("data/blob_{i}.bin");
        files.push((p, pseudo_random(2 * 1024 * 1024, 0x100 + i as u64)));
    }
    Workload { files }
}

struct Timings {
    write: Duration,
    cold_read: Duration,
    warm_read: Duration,
}

impl Timings {
    fn fmt(&self) -> String {
        format!(
            "write={:>8.2?}  cold_read={:>8.2?}  warm_read={:>8.2?}",
            self.write, self.cold_read, self.warm_read
        )
    }
}

/// Run the full write→cold-read→warm-read cycle against a given store and return
/// elapsed times for each phase.
async fn run(store: &FsStore, wl: &Workload) -> Timings {
    // ── write phase: write each file, then push a snapshot ────────────────
    let t0 = Instant::now();
    let mut mount = SessionMount::empty(store.clone());
    for (p, bytes) in &wl.files {
        mount.write(Path::new(p), bytes).await.unwrap();
    }
    let root = mount.push().await.unwrap();
    let write = t0.elapsed();

    // ── cold read: a fresh mount over the just-pushed root, read everything.
    let t0 = Instant::now();
    let m1 = SessionMount::pull(store.clone(), root).await.unwrap();
    for (p, _) in &wl.files {
        let _ = m1.read(Path::new(p)).await.unwrap();
    }
    let cold_read = t0.elapsed();

    // ── warm read: same mount (or an equivalent one over a warm cache) reads
    // the files again. With the write-through cache the chunks/nodes are now
    // local, so no primary round-trips occur.
    let t0 = Instant::now();
    let m2 = SessionMount::pull(store.clone(), root).await.unwrap();
    for (p, _) in &wl.files {
        let _ = m2.read(Path::new(p)).await.unwrap();
    }
    let warm_read = t0.elapsed();

    Timings {
        write,
        cold_read,
        warm_read,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "multi-second benchmark; run with --nocapture --ignored"]
async fn bench_write_through_cache_vs_remote_primary() {
    let wl = workload();

    // ── Baseline: latency primary ONLY (no local cache). ──────────────────
    // Every blob get/put pays LATENCY. This models an fs store backed directly
    // by S3 with no write-through cache in front.
    let primary = LatencyStorage {
        inner: MemoryHeapStorage::new(),
    };
    let store_nocache = FsStore::new(Arc::new(primary));
    let no_cache = run(&store_nocache, &wl).await;

    // ── Cached: same latency primary, wrapped by the write-through FS cache.
    let primary = LatencyStorage {
        inner: MemoryHeapStorage::new(),
    };
    let cache_dir = TempDir::new().unwrap();
    let cached = WriteThroughCacheHeapStorage::new(primary, cache_dir.path());
    let store_cache = FsStore::new(Arc::new(cached));
    let with_cache = run(&store_cache, &wl).await;

    println!();
    println!("============================================================");
    println!(" fs snapshot store: write-through cache benefit");
    println!(" modeled primary latency: {:?}", LATENCY);
    println!("============================================================");
    println!(" no-cache   : {}", no_cache.fmt());
    println!(" with-cache : {}", with_cache.fmt());
    println!("------------------------------------------------------------");
    println!(
        " write       speedup: {:>5.2}x",
        no_cache.write.as_secs_f64() / with_cache.write.as_secs_f64()
    );
    println!(
        " cold_read   speedup: {:>5.2}x",
        no_cache.cold_read.as_secs_f64() / with_cache.cold_read.as_secs_f64()
    );
    println!(
        " warm_read   speedup: {:>5.2}x",
        no_cache.warm_read.as_secs_f64() / with_cache.warm_read.as_secs_f64()
    );
    println!("============================================================");

    // Sanity: caching must not make reads slower than the cacheless baseline,
    // and a warm cache read should beat the cold (cache-miss-heavy) read.
    assert!(
        with_cache.warm_read <= no_cache.warm_read,
        "write-through cache made warm reads slower: cacheless={} cached={}",
        no_cache.warm_read.as_secs_f64(),
        with_cache.warm_read.as_secs_f64()
    );
    assert!(
        with_cache.warm_read <= with_cache.cold_read,
        "warm read should be faster than cold read: cold={} warm={}",
        with_cache.cold_read.as_secs_f64(),
        with_cache.warm_read.as_secs_f64()
    );
}
