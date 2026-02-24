//! Content-addressed buffer storage for pipeline outputs.
//!
//! Buffers (stdout, stderr, output) are stored as blobs in the same
//! HeapStorage backend (fs or S3) and indexed in sled for retrieval.

use serde::{Deserialize, Serialize};
use sha2::{Sha256, Digest};

use super::heap_storage::{HeapStorage, AnyHeapStorage};

/// Prefix for buffer blob keys in HeapStorage, to avoid collisions with
/// heap snapshots which use raw SHA-256 hex strings.
const BUFFER_PREFIX: &str = "buf:";

/// Sled tree name for the buffer index.
const BUFFER_TREE: &str = "__buffers__";

/// A reference to a stored buffer, returned to the caller instead of inline data.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BufferRef {
    /// SHA-256 content hash of the buffer data.
    pub hash: String,
    /// Size of the buffer in bytes.
    pub size: usize,
}

/// Metadata stored in sled for each buffer.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BufferMeta {
    pub hash: String,
    pub size: usize,
    pub timestamp: String,
}

#[derive(Clone)]
pub struct BufferStore {
    storage: AnyHeapStorage,
    db: sled::Db,
}

impl BufferStore {
    pub fn new(storage: AnyHeapStorage, db: sled::Db) -> Self {
        Self { storage, db }
    }

    /// Store a buffer and return its content-addressed reference.
    /// Empty buffers are not stored — returns None.
    pub async fn put(&self, data: &[u8]) -> Result<Option<BufferRef>, String> {
        if data.is_empty() {
            return Ok(None);
        }

        let hash = sha256_hex(data);
        let size = data.len();
        let storage_key = format!("{}{}", BUFFER_PREFIX, hash);

        // Store blob in HeapStorage (content-addressed = idempotent)
        self.storage.put(&storage_key, data).await?;

        // Index in sled
        let meta = BufferMeta {
            hash: hash.clone(),
            size,
            timestamp: chrono::Utc::now().to_rfc3339(),
        };
        let meta_bytes = serde_json::to_vec(&meta)
            .map_err(|e| format!("Failed to serialize buffer meta: {}", e))?;

        let tree = self.db.open_tree(BUFFER_TREE)
            .map_err(|e| format!("Failed to open buffer tree: {}", e))?;
        tree.insert(hash.as_bytes(), meta_bytes)
            .map_err(|e| format!("Failed to index buffer: {}", e))?;

        Ok(Some(BufferRef { hash, size }))
    }

    /// Store a string buffer, returning its reference.
    pub async fn put_string(&self, s: &str) -> Result<Option<BufferRef>, String> {
        if s.is_empty() {
            return Ok(None);
        }
        self.put(s.as_bytes()).await
    }

    /// Store a Vec<String> (e.g. stdout lines) as newline-joined bytes.
    pub async fn put_lines(&self, lines: &[String]) -> Result<Option<BufferRef>, String> {
        if lines.is_empty() {
            return Ok(None);
        }
        let joined = lines.join("\n");
        self.put(joined.as_bytes()).await
    }

    /// Retrieve a buffer by its content hash.
    pub async fn get(&self, hash: &str) -> Result<Vec<u8>, String> {
        let storage_key = format!("{}{}", BUFFER_PREFIX, hash);
        self.storage.get(&storage_key).await
    }

    /// Retrieve a buffer as a UTF-8 string.
    pub async fn get_string(&self, hash: &str) -> Result<String, String> {
        let data = self.get(hash).await?;
        String::from_utf8(data).map_err(|e| format!("Buffer is not valid UTF-8: {}", e))
    }
}

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let result = hasher.finalize();
    result.iter().map(|b| format!("{:02x}", b)).collect()
}
