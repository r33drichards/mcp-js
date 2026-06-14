use std::path::PathBuf;
use aws_sdk_s3::Client as S3Client;
use aws_sdk_s3::ByteStream;

use aws_config;
use std::sync::Arc;
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
        let config = aws_config::load_from_env().await;
        let client = S3Client::new(&config);
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

/// Write-through cache that wraps any primary HeapStorage with a local filesystem cache.
/// On put: writes to both the local FS cache and the primary storage.
/// On get: checks the FS cache first; falls back to the primary and caches the result locally.
#[derive(Clone)]
pub struct WriteThroughCacheHeapStorage<P: HeapStorage + Clone> {
    primary: P,
    cache: FileHeapStorage,
}

impl<P: HeapStorage + Clone> WriteThroughCacheHeapStorage<P> {
    pub fn new(primary: P, cache_dir: impl Into<PathBuf>) -> Self {
        Self {
            primary,
            cache: FileHeapStorage::new(cache_dir),
        }
    }
}

#[async_trait]
impl<P: HeapStorage + Clone> HeapStorage for WriteThroughCacheHeapStorage<P> {
    async fn put(&self, name: &str, data: &[u8]) -> Result<(), String> {
        // Write-through: write to both FS cache and primary
        self.cache.put(name, data).await?;
        self.primary.put(name, data).await?;
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
        }
        Ok(data)
    }
    async fn list(&self) -> Result<Vec<String>, String> {
        self.primary.list().await
    }
    async fn delete(&self, name: &str) -> Result<(), String> {
        // Remove from both layers; the cache delete is best-effort.
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