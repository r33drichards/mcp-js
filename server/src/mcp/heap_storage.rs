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

/// S3 storage with a local filesystem write-through cache.
/// On put: writes to both the local FS cache and S3.
/// On get: checks the FS cache first; falls back to S3 and caches the result locally.
#[derive(Clone)]
pub struct S3WithFsCacheHeapStorage {
    s3: S3HeapStorage,
    cache: FileHeapStorage,
}

impl S3WithFsCacheHeapStorage {
    pub async fn new(bucket: impl Into<String>, cache_dir: impl Into<PathBuf>) -> Self {
        Self {
            s3: S3HeapStorage::new(bucket).await,
            cache: FileHeapStorage::new(cache_dir),
        }
    }
}

#[async_trait]
impl HeapStorage for S3WithFsCacheHeapStorage {
    async fn put(&self, name: &str, data: &[u8]) -> Result<(), String> {
        // Write-through: write to both FS cache and S3
        self.cache.put(name, data).await?;
        self.s3.put(name, data).await?;
        Ok(())
    }
    async fn get(&self, name: &str) -> Result<Vec<u8>, String> {
        // Check FS cache first
        if let Ok(data) = self.cache.get(name).await {
            return Ok(data);
        }
        // Cache miss: fetch from S3 and populate cache
        let data = self.s3.get(name).await?;
        // Best-effort cache population; don't fail if local write fails
        if let Err(e) = self.cache.put(name, &data).await {
            tracing::warn!("Failed to populate FS cache for {}: {}", name, e);
        }
        Ok(data)
    }
}

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
} 