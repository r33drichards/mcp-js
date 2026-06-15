use std::path::PathBuf;
use aws_sdk_s3::Client as S3Client;
use aws_sdk_s3::primitives::ByteStream;

use aws_config;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use async_trait::async_trait;

#[async_trait]
pub trait HeapStorage: Send + Sync + 'static {
    async fn put(&self, name: &str, data: &[u8]) -> Result<(), String>;
    async fn get(&self, name: &str) -> Result<Vec<u8>, String>;

    /// List every blob name in the backend. Used by fs snapshot GC. Backends
    /// that cannot enumerate return an error (GC is then unavailable on them).
    async fn list(&self) -> Result<Vec<String>, String> {
        Err("list is not supported by this storage backend".to_string())
    }

    /// Delete a blob by name. Used by fs snapshot GC sweep.
    async fn delete(&self, _name: &str) -> Result<(), String> {
        Err("delete is not supported by this storage backend".to_string())
    }
}

/// In-memory blob backend. Primarily for tests and the `FsStore::in_memory`
/// constructor, but kept in the normal build so integration test crates (which
/// compile the lib without `--cfg test`) can use it.
#[derive(Clone, Default)]
pub struct MemoryHeapStorage {
    map: Arc<std::sync::Mutex<std::collections::HashMap<String, Vec<u8>>>>,
}

impl MemoryHeapStorage {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl HeapStorage for MemoryHeapStorage {
    async fn put(&self, name: &str, data: &[u8]) -> Result<(), String> {
        self.map
            .lock()
            .unwrap()
            .insert(name.to_string(), data.to_vec());
        Ok(())
    }
    async fn get(&self, name: &str) -> Result<Vec<u8>, String> {
        self.map
            .lock()
            .unwrap()
            .get(name)
            .cloned()
            .ok_or_else(|| format!("not found: {name}"))
    }
    async fn list(&self) -> Result<Vec<String>, String> {
        Ok(self.map.lock().unwrap().keys().cloned().collect())
    }
    async fn delete(&self, name: &str) -> Result<(), String> {
        self.map.lock().unwrap().remove(name);
        Ok(())
    }
}

#[derive(Clone)]
pub struct FileHeapStorage {
    dir: PathBuf,
}

impl FileHeapStorage {

    pub fn new(dir: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        std::fs::create_dir_all(&dir).ok();
        Self { dir }
    }
}

#[async_trait]
impl HeapStorage for FileHeapStorage {
    async fn put(&self, name: &str, data: &[u8]) -> Result<(), String> {
        let path = self.dir.join(name);
        std::fs::write(path, data).map_err(|e| e.to_string())
    }
    async fn get(&self, name: &str) -> Result<Vec<u8>, String> {
        let path = self.dir.join(name);
        std::fs::read(path).map_err(|e| e.to_string())
    }
    async fn list(&self) -> Result<Vec<String>, String> {
        let mut out = Vec::new();
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(e) => e,
            // A backend dir that was never written to has nothing to list.
            Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(e) => return Err(e.to_string()),
        };
        for entry in entries {
            let entry = entry.map_err(|e| e.to_string())?;
            if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                if let Some(name) = entry.file_name().to_str() {
                    out.push(name.to_string());
                }
            }
        }
        Ok(out)
    }
    async fn delete(&self, name: &str) -> Result<(), String> {
        let path = self.dir.join(name);
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

#[derive(Clone)]
pub struct S3HeapStorage {
    bucket: String,
    client: Arc<S3Client>,
}

impl S3HeapStorage {
    pub async fn new(bucket: impl Into<String>) -> Self {
        let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .load()
            .await;
        let mut s3_builder = aws_sdk_s3::config::Builder::from(&config);

        // Point at an S3-compatible store (MinIO, etc.) when a custom endpoint
        // is configured. The modern SDK also honors AWS_ENDPOINT_URL on its own,
        // but set it explicitly so behavior is independent of loader specifics.
        if let Ok(endpoint) = std::env::var("AWS_ENDPOINT_URL") {
            if !endpoint.is_empty() {
                s3_builder = s3_builder.endpoint_url(endpoint);
            }
        }

        // Path-style addressing — required by most S3-compatible stores
        // (MinIO and friends serve `host/bucket/key`, not `bucket.host/key`).
        // Real AWS uses virtual-hosted style, so this stays off unless asked.
        let force_path_style = std::env::var("AWS_S3_FORCE_PATH_STYLE")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);
        if force_path_style {
            s3_builder = s3_builder.force_path_style(true);
        }

        let client = S3Client::from_conf(s3_builder.build());
        Self {
            bucket: bucket.into(),
            client: Arc::new(client),
        }
    }

    async fn put_blocking(&self, name: &str, data: &[u8]) -> Result<(), String> {
        let client = self.client.clone();
        let bucket = self.bucket.clone();
        let name = name.to_string();
        let data = data.to_vec();
        client
            .put_object()
            .bucket(bucket)
            .key(name)
            .body(ByteStream::from(data))
            .send()
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    async fn get_blocking(&self, name: &str) -> Result<Vec<u8>, String> {
        let client = self.client.clone();
        let bucket = self.bucket.clone();
        let name = name.to_string();
        let output = client
            .get_object()
            .bucket(bucket)
            .key(name)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let data = output.body.collect().await.map_err(|e| e.to_string())?;
        Ok(data.into_bytes().to_vec())
    }
}

#[async_trait]
impl HeapStorage for S3HeapStorage {
    async fn put(&self, name: &str, data: &[u8]) -> Result<(), String> {
        self.put_blocking(name, data).await
    }
    async fn get(&self, name: &str) -> Result<Vec<u8>, String> {
        self.get_blocking(name).await
    }
}

/// Default local-cache capacity (8 GiB). Large enough that small/medium working
/// sets stay fully cached, bounded enough that a multi-TB primary never tries to
/// mirror itself onto local disk.
pub const DEFAULT_CACHE_CAPACITY_BYTES: u64 = 8 * 1024 * 1024 * 1024;

/// Size-bounded record of what currently lives in the local cache. Eviction is
/// FIFO by insertion (a simple, allocation-free approximation of LRU); the
/// primary remains the source of truth, so an evicted blob is simply re-fetched
/// and re-cached on the next `get`.
#[derive(Default)]
struct CacheBound {
    sizes: HashMap<String, u64>,
    order: VecDeque<String>,
    total: u64,
    cap: u64,
}

impl CacheBound {
    /// Record `name` at `size` bytes and return the keys that must be evicted
    /// from the local cache to stay within `cap`.
    fn record(&mut self, name: &str, size: u64) -> Vec<String> {
        if let Some(old) = self.sizes.insert(name.to_string(), size) {
            self.total -= old;
        } else {
            self.order.push_back(name.to_string());
        }
        self.total += size;

        let mut evicted = Vec::new();
        while self.total > self.cap {
            // Never evict the entry we just recorded (it is at the back).
            if self.order.len() <= 1 {
                break;
            }
            let Some(victim) = self.order.pop_front() else { break };
            if let Some(sz) = self.sizes.remove(&victim) {
                self.total -= sz;
            }
            evicted.push(victim);
        }
        evicted
    }

    fn forget(&mut self, name: &str) {
        if let Some(sz) = self.sizes.remove(name) {
            self.total -= sz;
            if let Some(pos) = self.order.iter().position(|k| k == name) {
                self.order.remove(pos);
            }
        }
    }
}

/// Write-through cache that wraps any primary HeapStorage with a **size-bounded**
/// local filesystem cache.
/// On put: writes to both the local FS cache and the primary storage.
/// On get: checks the FS cache first; falls back to the primary and caches the
/// result locally. When the cache exceeds its capacity, the oldest entries are
/// evicted from local disk (the primary is untouched, so reads still succeed).
#[derive(Clone)]
pub struct WriteThroughCacheHeapStorage<P: HeapStorage + Clone> {
    primary: P,
    cache: FileHeapStorage,
    bound: Arc<Mutex<CacheBound>>,
}

impl<P: HeapStorage + Clone> WriteThroughCacheHeapStorage<P> {
    pub fn new(primary: P, cache_dir: impl Into<PathBuf>) -> Self {
        Self::with_capacity_bytes(primary, cache_dir, DEFAULT_CACHE_CAPACITY_BYTES)
    }

    /// Build a write-through cache capped at `cap_bytes` of resident local data.
    pub fn with_capacity_bytes(primary: P, cache_dir: impl Into<PathBuf>, cap_bytes: u64) -> Self {
        Self {
            primary,
            cache: FileHeapStorage::new(cache_dir),
            bound: Arc::new(Mutex::new(CacheBound {
                cap: cap_bytes.max(1),
                ..Default::default()
            })),
        }
    }

    /// Record a freshly-cached blob and evict the oldest entries (from the local
    /// cache only) to stay within capacity. Eviction is best-effort.
    async fn note_cached(&self, name: &str, size: u64) {
        let evicted = self.bound.lock().unwrap().record(name, size);
        for key in evicted {
            let _ = self.cache.delete(&key).await;
        }
    }
}

#[async_trait]
impl<P: HeapStorage + Clone> HeapStorage for WriteThroughCacheHeapStorage<P> {
    async fn put(&self, name: &str, data: &[u8]) -> Result<(), String> {
        // Write-through: write to both FS cache and primary
        self.cache.put(name, data).await?;
        self.primary.put(name, data).await?;
        self.note_cached(name, data.len() as u64).await;
        Ok(())
    }
    async fn get(&self, name: &str) -> Result<Vec<u8>, String> {
        // Check FS cache first
        if let Ok(data) = self.cache.get(name).await {
            return Ok(data);
        }
        // Cache miss: fetch from primary and populate cache
        let data = self.primary.get(name).await?;
        // Best-effort cache population; don't fail if local write fails
        if let Err(e) = self.cache.put(name, &data).await {
            tracing::warn!("Failed to populate FS cache for {}: {}", name, e);
        } else {
            self.note_cached(name, data.len() as u64).await;
        }
        Ok(data)
    }
    async fn list(&self) -> Result<Vec<String>, String> {
        self.primary.list().await
    }
    async fn delete(&self, name: &str) -> Result<(), String> {
        // Remove from both layers; the cache delete is best-effort.
        self.bound.lock().unwrap().forget(name);
        let _ = self.cache.delete(name).await;
        self.primary.delete(name).await
    }
}

/// S3 with local filesystem write-through cache.
pub type S3WithFsCacheHeapStorage = WriteThroughCacheHeapStorage<S3HeapStorage>;

#[derive(Clone)]
pub enum AnyHeapStorage {
    File(FileHeapStorage),
    S3(S3HeapStorage),
    S3WithFsCache(S3WithFsCacheHeapStorage),
}




#[async_trait::async_trait]
impl HeapStorage for AnyHeapStorage {
    async fn put(&self, name: &str, data: &[u8]) -> Result<(), String> {
        match self {
            AnyHeapStorage::File(inner) => inner.put(name, data).await,
            AnyHeapStorage::S3(inner) => inner.put(name, data).await,
            AnyHeapStorage::S3WithFsCache(inner) => inner.put(name, data).await,
        }
    }
    async fn get(&self, name: &str) -> Result<Vec<u8>, String> {
        match self {
            AnyHeapStorage::File(inner) => inner.get(name).await,
            AnyHeapStorage::S3(inner) => inner.get(name).await,
            AnyHeapStorage::S3WithFsCache(inner) => inner.get(name).await,
        }
    }
    async fn list(&self) -> Result<Vec<String>, String> {
        match self {
            AnyHeapStorage::File(inner) => inner.list().await,
            AnyHeapStorage::S3(inner) => inner.list().await,
            AnyHeapStorage::S3WithFsCache(inner) => inner.list().await,
        }
    }
    async fn delete(&self, name: &str) -> Result<(), String> {
        match self {
            AnyHeapStorage::File(inner) => inner.delete(name).await,
            AnyHeapStorage::S3(inner) => inner.delete(name).await,
            AnyHeapStorage::S3WithFsCache(inner) => inner.delete(name).await,
        }
    }
}