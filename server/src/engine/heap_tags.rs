use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

use crate::cluster::ClusterNode;

const HT_PREFIX: &str = "ht:";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeapTagEntry {
    pub heap: String,
    pub tags: HashMap<String, String>,
}

#[derive(Clone)]
pub struct HeapTagStore {
    db: sled::Db,
    cluster_node: Option<Arc<ClusterNode>>,
}

impl HeapTagStore {
    pub fn new(path: &str) -> Result<Self, String> {
        let db = sled::open(path).map_err(|e| format!("Failed to open sled db: {}", e))?;
        Ok(Self {
            db,
            cluster_node: None,
        })
    }

    pub fn from_config(config: sled::Config) -> Result<Self, String> {
        let db = config
            .open()
            .map_err(|e| format!("Failed to open sled db: {}", e))?;
        Ok(Self {
            db,
            cluster_node: None,
        })
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

        let tree = self
            .db
            .open_tree("heap_tags")
            .map_err(|e| format!("Failed to open tree: {}", e))?;
        tree.insert(key.as_bytes(), value.as_bytes())
            .map_err(|e| format!("Failed to insert tags: {}", e))?;
        Ok(())
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

        let tree = self
            .db
            .open_tree("heap_tags")
            .map_err(|e| format!("Failed to open tree: {}", e))?;
        match tree.get(key.as_bytes()) {
            Ok(Some(val)) => {
                let tags: HashMap<String, String> = serde_json::from_slice(&val)
                    .map_err(|e| format!("Failed to deserialize tags: {}", e))?;
                Ok(tags)
            }
            Ok(None) => Ok(HashMap::new()),
            Err(e) => Err(format!("Failed to get tags: {}", e)),
        }
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

        let tree = self
            .db
            .open_tree("heap_tags")
            .map_err(|e| format!("Failed to open tree: {}", e))?;
        for item in tree.scan_prefix(HT_PREFIX.as_bytes()) {
            let (key_bytes, val_bytes) =
                item.map_err(|e| format!("Failed to read entry: {}", e))?;
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
