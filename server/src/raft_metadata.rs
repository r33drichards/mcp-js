//! Optional binary metadata adapter over the existing Raft state machine.
//! Reads are local committed reads; a pending CAS mismatch remains provisional.
use crate::cluster::{CasRequest, ClusterNode, LogEntry};
use mcp_metadata_store::{MetadataStore, Result, Row, Write};
use std::{collections::BTreeSet, sync::Arc};

pub struct RaftMetadataStore {
    node: Arc<ClusterNode>,
    prefix: String,
}
impl RaftMetadataStore {
    pub fn new(node: Arc<ClusterNode>, namespace: &str) -> Result<Self> {
        if namespace.is_empty() || namespace.len() > 128 {
            return Err("invalid Raft metadata namespace".into());
        }
        Ok(Self {
            node,
            prefix: format!("metadata:v1:{}:", hex::encode(namespace)),
        })
    }
    fn tree_prefix(&self, tree: &str) -> String {
        format!("{}t:{}:k:", self.prefix, hex::encode(tree))
    }
    fn key(&self, tree: &str, key: &[u8]) -> String {
        format!("{}{}", self.tree_prefix(tree), hex::encode(key))
    }
    fn prepare(
        &self,
        primary: Write,
        companions: Vec<Write>,
    ) -> Result<(String, String, Vec<(String, String)>)> {
        let key = self.key(&primary.tree, &primary.key);
        let value = hex::encode(primary.value);
        let extra: Vec<_> = companions
            .into_iter()
            .map(|w| (self.key(&w.tree, &w.key), hex::encode(w.value)))
            .collect();
        let mut keys = BTreeSet::from([key.clone()]);
        if extra.iter().any(|(k, _)| !keys.insert(k.clone())) {
            return Err("duplicate transaction key".into());
        }
        let encoded = serde_json::to_vec(&LogEntry {
            term: u64::MAX,
            index: u64::MAX,
            key: key.clone(),
            value: value.clone(),
            extra: extra.clone(),
        })
        .map_err(|e| e.to_string())?;
        if encoded.len() > crate::cluster::MAX_APPEND_BYTES {
            return Err("Raft metadata entry exceeds replication size limit".into());
        }
        Ok((key, value, extra))
    }
}
#[async_trait::async_trait]
impl MetadataStore for RaftMetadataStore {
    fn validate_transaction(
        &self,
        primary: &Write,
        expected: Option<&[u8]>,
        companions: &[Write],
    ) -> Result<()> {
        let (key, value, extra) = self.prepare(primary.clone(), companions.to_vec())?;
        let request = CasRequest {
            key,
            expected: expected.map(hex::encode),
            new: value,
            extra,
        };
        if serde_json::to_vec(&request)
            .map_err(|e| e.to_string())?
            .len()
            > crate::cluster::MAX_RAFT_BODY_BYTES
        {
            return Err("Raft metadata request exceeds RPC size limit".into());
        }
        Ok(())
    }
    fn supports_linearizable_reads(&self) -> bool {
        false
    }
    fn provisional_conflicts(&self) -> bool {
        true
    }
    async fn get(&self, tree: &str, key: &[u8]) -> Result<Option<Vec<u8>>> {
        self.node
            .get(&self.key(tree, key))
            .await?
            .map(hex::decode)
            .transpose()
            .map_err(|e| e.to_string())
    }
    async fn put(&self, write: Write) -> Result<()> {
        let (key, value, extra) = self.prepare(write, vec![])?;
        self.node.put_with_or_forward(key, value, extra).await
    }
    async fn scan(
        &self,
        tree: &str,
        prefix: &[u8],
        reverse: bool,
        limit: Option<usize>,
    ) -> Result<Vec<Row>> {
        let tree_prefix = self.tree_prefix(tree);
        let mut rows = self
            .node
            .scan_prefix(&format!("{}{}", tree_prefix, hex::encode(prefix)))?
            .into_iter()
            .map(|(key, value)| {
                Ok((
                    hex::decode(&key[tree_prefix.len()..]).map_err(|e| e.to_string())?,
                    hex::decode(value).map_err(|e| e.to_string())?,
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        if reverse {
            rows.reverse()
        }
        if let Some(limit) = limit {
            rows.truncate(limit)
        }
        Ok(rows)
    }
    async fn compare_and_swap(
        &self,
        primary: Write,
        expected: Option<Vec<u8>>,
        companions: Vec<Write>,
    ) -> Result<bool> {
        let (key, value, extra) = self.prepare(primary, companions)?;
        let expected = expected.map(hex::encode);
        let request = CasRequest {
            key: key.clone(),
            expected: expected.clone(),
            new: value.clone(),
            extra: extra.clone(),
        };
        if serde_json::to_vec(&request)
            .map_err(|e| e.to_string())?
            .len()
            > crate::cluster::MAX_RAFT_BODY_BYTES
        {
            return Err("Raft metadata request exceeds RPC size limit".into());
        }
        // A pending-state mismatch is exposed through provisional_conflicts().
        self.node
            .cas_with_or_forward(key, expected, value, extra)
            .await
    }
    async fn generate_id(&self) -> Result<u64> {
        let key = format!("{}counter", self.prefix);
        for _ in 0..1000 {
            let old = self.node.get(&key).await?;
            let current = old
                .as_deref()
                .map(|v| {
                    let bytes = hex::decode(v).map_err(|e| e.to_string())?;
                    Ok::<_, String>(u64::from_be_bytes(
                        bytes.try_into().map_err(|_| "invalid replicated counter")?,
                    ))
                })
                .transpose()?
                .unwrap_or(0);
            let next = current
                .checked_add(1)
                .ok_or("replicated ID counter exhausted")?;
            if self
                .node
                .cas_or_forward(key.clone(), old, hex::encode(next.to_be_bytes()))
                .await?
            {
                return Ok(next);
            }
            tokio::task::yield_now().await;
        }
        Err("replicated ID contention; retry allocation".into())
    }
    async fn tree_names(&self) -> Result<Vec<String>> {
        self.node
            .scan_prefix(&format!("{}t:", self.prefix))?
            .into_iter()
            .map(|(key, _)| {
                let tree = key
                    .strip_prefix(&format!("{}t:", self.prefix))
                    .and_then(|s| s.split_once(":k:"))
                    .ok_or("invalid Raft metadata key")?
                    .0;
                String::from_utf8(hex::decode(tree).map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())
            })
            .collect::<Result<BTreeSet<_>>>()
            .map(|trees| trees.into_iter().collect())
    }
    fn flush(&self) -> Result<()> {
        self.node.db.flush().map(|_| ()).map_err(|e| e.to_string())
    }
}
