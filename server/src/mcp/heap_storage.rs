use std::path::PathBuf;
use aws_sdk_s3::Client as S3Client;
use aws_sdk_s3::ByteStream;

use aws_config;
use std::sync::Arc;
use async_trait::async_trait;
use std::collections::HashMap;
use tokio::sync::RwLock;

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

    // ignore dead_code warning
    #[allow(dead_code)]
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

/// In-memory heap storage using HashMap
/// Perfect for debugging, testing, or when persistence isn't needed
#[derive(Clone)]
pub struct InMemoryHeapStorage {
    store: Arc<RwLock<HashMap<String, Vec<u8>>>>,
}

impl InMemoryHeapStorage {
    pub fn new() -> Self {
        Self {
            store: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Get the number of heaps currently stored
    pub async fn len(&self) -> usize {
        self.store.read().await.len()
    }

    /// Check if the store is empty
    pub async fn is_empty(&self) -> bool {
        self.store.read().await.is_empty()
    }

    /// Clear all stored heaps
    pub async fn clear(&self) {
        self.store.write().await.clear();
    }

    /// List all heap names
    pub async fn list_heaps(&self) -> Vec<String> {
        self.store.read().await.keys().cloned().collect()
    }
}

impl Default for InMemoryHeapStorage {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl HeapStorage for InMemoryHeapStorage {
    async fn put(&self, name: &str, data: &[u8]) -> Result<(), String> {
        let mut store = self.store.write().await;
        store.insert(name.to_string(), data.to_vec());
        tracing::debug!("Stored heap '{}' ({} bytes) in memory", name, data.len());
        Ok(())
    }

    async fn get(&self, name: &str) -> Result<Vec<u8>, String> {
        let store = self.store.read().await;
        store
            .get(name)
            .cloned()
            .ok_or_else(|| format!("Heap '{}' not found in memory", name))
    }
}

#[derive(Clone)]
pub enum AnyHeapStorage {
    #[allow(dead_code)]
    File(FileHeapStorage),
    S3(S3HeapStorage),
    InMemory(InMemoryHeapStorage),
}




#[async_trait::async_trait]
impl HeapStorage for AnyHeapStorage {
    async fn put(&self, name: &str, data: &[u8]) -> Result<(), String> {
        match self {
            AnyHeapStorage::File(inner) => inner.put(name, data).await,
            AnyHeapStorage::S3(inner) => inner.put(name, data).await,
            AnyHeapStorage::InMemory(inner) => inner.put(name, data).await,
        }
    }
    async fn get(&self, name: &str) -> Result<Vec<u8>, String> {
        match self {
            AnyHeapStorage::File(inner) => inner.get(name).await,
            AnyHeapStorage::S3(inner) => inner.get(name).await,
            AnyHeapStorage::InMemory(inner) => inner.get(name).await,
        }
    }
} 