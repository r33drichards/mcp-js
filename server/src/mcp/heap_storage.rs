use std::path::PathBuf;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use aws_sdk_s3::Client as S3Client;
use aws_sdk_s3::ByteStream;
use aws_config;
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

/// In-memory heap storage using a HashMap protected by RwLock
/// This is useful for testing and development, or when persistence is not required
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

    /// Get the number of items currently stored
    pub async fn len(&self) -> usize {
        self.store.read().await.len()
    }

    /// Check if the storage is empty
    pub async fn is_empty(&self) -> bool {
        self.store.read().await.is_empty()
    }

    /// Clear all stored data
    pub async fn clear(&self) {
        self.store.write().await.clear();
    }

    /// Check if a key exists
    pub async fn contains_key(&self, name: &str) -> bool {
        self.store.read().await.contains_key(name)
    }

    /// Remove a specific key
    pub async fn remove(&self, name: &str) -> Option<Vec<u8>> {
        self.store.write().await.remove(name)
    }

    /// Get all keys currently stored
    pub async fn keys(&self) -> Vec<String> {
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
        Ok(())
    }

    async fn get(&self, name: &str) -> Result<Vec<u8>, String> {
        let store = self.store.read().await;
        store
            .get(name)
            .cloned()
            .ok_or_else(|| format!("Key '{}' not found in memory storage", name))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_in_memory_storage_basic_operations() {
        let storage = InMemoryHeapStorage::new();

        // Test put and get
        let data = b"hello world";
        storage.put("test_key", data).await.unwrap();
        let retrieved = storage.get("test_key").await.unwrap();
        assert_eq!(retrieved, data);

        // Test get non-existent key
        let result = storage.get("non_existent").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_in_memory_storage_operations() {
        let storage = InMemoryHeapStorage::new();

        // Test empty
        assert!(storage.is_empty().await);
        assert_eq!(storage.len().await, 0);

        // Add data
        storage.put("key1", b"value1").await.unwrap();
        storage.put("key2", b"value2").await.unwrap();

        // Test len and is_empty
        assert!(!storage.is_empty().await);
        assert_eq!(storage.len().await, 2);

        // Test contains_key
        assert!(storage.contains_key("key1").await);
        assert!(!storage.contains_key("key3").await);

        // Test keys
        let keys = storage.keys().await;
        assert_eq!(keys.len(), 2);
        assert!(keys.contains(&"key1".to_string()));
        assert!(keys.contains(&"key2".to_string()));

        // Test remove
        let removed = storage.remove("key1").await;
        assert!(removed.is_some());
        assert_eq!(removed.unwrap(), b"value1");
        assert_eq!(storage.len().await, 1);

        // Test clear
        storage.clear().await;
        assert!(storage.is_empty().await);
    }

    #[tokio::test]
    async fn test_in_memory_storage_clone() {
        let storage1 = InMemoryHeapStorage::new();
        storage1.put("key", b"value").await.unwrap();

        // Clone should share the same underlying storage
        let storage2 = storage1.clone();
        let retrieved = storage2.get("key").await.unwrap();
        assert_eq!(retrieved, b"value");

        // Changes through clone should be visible
        storage2.put("key2", b"value2").await.unwrap();
        assert!(storage1.contains_key("key2").await);
    }

    #[tokio::test]
    async fn test_any_heap_storage_in_memory() {
        let storage = AnyHeapStorage::InMemory(InMemoryHeapStorage::new());

        // Test through trait interface
        storage.put("test", b"data").await.unwrap();
        let retrieved = storage.get("test").await.unwrap();
        assert_eq!(retrieved, b"data");
    }
} 