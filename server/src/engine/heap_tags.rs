use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

use crate::cluster::ClusterNode;
use mcp_metadata_store::{MetadataStore, SledStore, Write};

const HT_PREFIX: &str = "ht:";

#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
pub struct HeapTagEntry {
    pub heap: String,
    pub tags: HashMap<String, String>,
}

#[derive(Clone)]
pub struct HeapTagStore {
    db: Arc<dyn MetadataStore>,
    cluster_node: Option<Arc<ClusterNode>>,
}

impl HeapTagStore {
    pub fn new(path: &str) -> Result<Self, String> {
        let db = sled::open(path).map_err(|e| format!("Failed to open sled db: {}", e))?;
        Ok(Self {
            db: Arc::new(SledStore(db)),
            cluster_node: None,
        })
    }

    pub fn from_config(config: sled::Config) -> Result<Self, String> {
        let db = config
            .open()
            .map_err(|e| format!("Failed to open sled db: {}", e))?;
        Ok(Self {
            db: Arc::new(SledStore(db)),
            cluster_node: None,
        })
    }

    pub fn from_store(db: Arc<dyn MetadataStore>) -> Self {
        Self { db, cluster_node: None }
    }

    pub fn with_cluster(mut self, node: Arc<ClusterNode>) -> Self {
        self.cluster_node = Some(node);
        self
    }

    fn make_key(heap: &str) -> String {
        format!("{}{}", HT_PREFIX, heap)
    }

    /// Replace all tags for a heap. Pass an empty map to clear tags.
    pub async fn set_tags(&self, heap: &str, tags: HashMap<String, String>) -> Result<(), String> {
        let key = Self::make_key(heap);
        let value =
            serde_json::to_string(&tags).map_err(|e| format!("Failed to serialize tags: {}", e))?;

        if let Some(ref cluster) = self.cluster_node {
            cluster.put_or_forward(key, value).await?;
            return Ok(());
        }

        self.db.put(Write::new("heap_tags", key.as_bytes(), value.into_bytes())).await
    }

    /// Get all tags for a heap. Returns empty map if no tags exist.
    pub async fn get_tags(&self, heap: &str) -> Result<HashMap<String, String>, String> {
        let key = Self::make_key(heap);

        if let Some(ref cluster) = self.cluster_node {
            let pairs = cluster.scan_prefix(&key)?;
            // Exact match: only the entry whose key equals our key
            for (k, v) in pairs {
                if k == key {
                    let tags: HashMap<String, String> = serde_json::from_str(&v)
                        .map_err(|e| format!("Failed to deserialize tags: {}", e))?;
                    return Ok(tags);
                }
            }
            return Ok(HashMap::new());
        }

        match self.db.get("heap_tags", key.as_bytes()).await? {
            Some(v) => serde_json::from_slice(&v).map_err(|e| e.to_string()),
            None => Ok(HashMap::new()),
        }
    }

    /// Atomically merge tags into a heap's existing tags.
    /// Existing keys not in `tags` are preserved. Keys in `tags` overwrite existing values.
    /// This is safe for concurrent use — uses backend compare-and-swap.
    pub async fn merge_tags(
        &self,
        heap: &str,
        tags: HashMap<String, String>,
    ) -> Result<(), String> {
        let key = Self::make_key(heap);

        if let Some(ref cluster) = self.cluster_node {
            // In cluster mode, read-modify-write through the cluster.
            // The Raft log serializes writes, but two concurrent merge_tags
            // may read stale data. This is a known limitation — last writer wins
            // for same-key conflicts, but different keys may be lost.
            let existing = self.get_tags(heap).await?;
            let mut merged = existing;
            merged.extend(tags);
            let value = serde_json::to_string(&merged)
                .map_err(|e| format!("Failed to serialize tags: {}", e))?;
            cluster.put_or_forward(key, value).await?;
            return Ok(());
        }

        for _ in 0..64 {
            let old = self.db.get("heap_tags", key.as_bytes()).await?;
            let mut existing: HashMap<String, String> = match &old {
                Some(v) => serde_json::from_slice(v).map_err(|e| e.to_string())?,
                None => HashMap::new(),
            };
            existing.extend(tags.clone());
            let value = serde_json::to_vec(&existing).map_err(|e| e.to_string())?;
            if self.db.compare_and_swap(Write::new("heap_tags", key.as_bytes(), value), old, vec![]).await? { return Ok(()); }
        }
        Err("heap tag contention: retry merge".into())
    }

    /// Delete tags from a heap. If `keys` is None, delete all tags.
    /// If `keys` is Some, delete only the specified keys.
    pub async fn delete_tags(
        &self,
        heap: &str,
        keys: Option<Vec<String>>,
    ) -> Result<(), String> {
        match keys {
            None => {
                // Delete all tags — write empty map as tombstone
                self.set_tags(heap, HashMap::new()).await
            }
            Some(keys_to_remove) => {
                // Read-modify-write: remove specific keys
                let mut tags = self.get_tags(heap).await?;
                for k in &keys_to_remove {
                    tags.remove(k);
                }
                self.set_tags(heap, tags).await
            }
        }
    }

    /// Find heaps whose tags contain all the specified key-value pairs.
    pub async fn query_by_tags(
        &self,
        filter: HashMap<String, String>,
    ) -> Result<Vec<HeapTagEntry>, String> {
        let mut results = Vec::new();

        if let Some(ref cluster) = self.cluster_node {
            let pairs = cluster.scan_prefix(HT_PREFIX)?;
            for (key, val) in pairs {
                let heap = key[HT_PREFIX.len()..].to_string();
                let tags: HashMap<String, String> = serde_json::from_str(&val)
                    .map_err(|e| format!("Failed to deserialize tags: {}", e))?;
                if !tags.is_empty() && filter.iter().all(|(k, v)| tags.get(k) == Some(v)) {
                    results.push(HeapTagEntry { heap, tags });
                }
            }
            return Ok(results);
        }

        for (key_bytes, val_bytes) in self.db.scan("heap_tags", HT_PREFIX.as_bytes(), false, None).await? {
            let key_str = String::from_utf8_lossy(&key_bytes);
            let heap = key_str[HT_PREFIX.len()..].to_string();
            let tags: HashMap<String, String> = serde_json::from_slice(&val_bytes)
                .map_err(|e| format!("Failed to deserialize tags: {}", e))?;
            if !tags.is_empty() && filter.iter().all(|(k, v)| tags.get(k) == Some(v)) {
                results.push(HeapTagEntry { heap, tags });
            }
        }

        Ok(results)
    }
}
