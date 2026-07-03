use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::cluster::ClusterNode;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionLogEntry {
    pub input_heap: Option<String>,
    pub output_heap: String,
    /// Resulting filesystem snapshot CA id (hex), independent of the heap.
    /// `None` for executions that ran without a mount. Defaults for entries
    /// written before fs snapshots existed.
    #[serde(default)]
    pub output_fs: Option<String>,
    pub code: String,
    pub timestamp: String, // ISO 8601 UTC
}

/// Result of [`SessionLog::fork`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForkOutcome {
    /// Forked: the target session now starts from these heap/fs snapshots.
    Forked { heap: String, fs: Option<String> },
    /// Target already had history; nothing was changed.
    TargetExists,
    /// Source session had no history to fork from.
    SourceEmpty,
}

fn now_iso8601() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Key prefixes used when storing session log data in the Raft-replicated
/// data tree.
const SL_SESSION_PREFIX: &str = "sl:s:";
const SL_ENTRY_PREFIX: &str = "sl:e:";

#[derive(Clone)]
pub struct SessionLog {
    /// Local sled database – used in standalone (non-cluster) mode and as
    /// fallback if the cluster write fails.
    db: sled::Db,
    /// When set, writes are routed through Raft consensus and reads come
    /// from the cluster's replicated data tree.
    cluster_node: Option<Arc<ClusterNode>>,
}

impl SessionLog {
    pub fn new(path: &str) -> Result<Self, String> {
        let db = sled::open(path).map_err(|e| format!("Failed to open sled db: {}", e))?;
        Ok(Self {
            db,
            cluster_node: None,
        })
    }

    pub fn from_config(config: sled::Config) -> Result<Self, String> {
        let db = config.open().map_err(|e| format!("Failed to open sled db: {}", e))?;
        Ok(Self {
            db,
            cluster_node: None,
        })
    }

    /// Attach a cluster node so that subsequent writes go through Raft.
    pub fn with_cluster(mut self, node: Arc<ClusterNode>) -> Self {
        self.cluster_node = Some(node);
        self
    }

    // --------------------------------------------------------------------
    // Internal helpers for the cluster key scheme
    // --------------------------------------------------------------------

    fn make_session_key(session: &str) -> String {
        format!("{}{}", SL_SESSION_PREFIX, session)
    }

    fn make_entry_key(session: &str) -> String {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        format!("{}{}:{:020}", SL_ENTRY_PREFIX, session, nanos)
    }

    fn entry_prefix_for_session(session: &str) -> String {
        format!("{}{}:", SL_ENTRY_PREFIX, session)
    }

    // --------------------------------------------------------------------
    // Public API – all async to accommodate cluster round-trips.
    // --------------------------------------------------------------------

    /// Append a log entry to the given session. Returns the sequence number.
    pub async fn append(&self, session: &str, entry: SessionLogEntry) -> Result<u64, String> {
        if let Some(ref cluster) = self.cluster_node {
            let value = serde_json::to_string(&entry)
                .map_err(|e| format!("Failed to serialize entry: {}", e))?;

            // Register the session name (idempotent).
            cluster
                .put_or_forward(Self::make_session_key(session), "1".to_string())
                .await?;

            // Store the entry itself.
            let entry_key = Self::make_entry_key(session);
            cluster.put_or_forward(entry_key, value).await?;

            // The sequence number is meaningless in cluster mode, but
            // callers only use it for logging so return a placeholder.
            return Ok(0);
        }

        // Standalone mode – write directly to local sled.
        let tree = self
            .db
            .open_tree(session)
            .map_err(|e| format!("Failed to open tree '{}': {}", session, e))?;

        let value =
            serde_json::to_vec(&entry).map_err(|e| format!("Failed to serialize entry: {}", e))?;

        let seq = self
            .db
            .generate_id()
            .map_err(|e| format!("Failed to generate id: {}", e))?;

        let key = seq.to_be_bytes();
        tree.insert(key, value)
            .map_err(|e| format!("Failed to insert entry: {}", e))?;

        Ok(seq)
    }

    /// List all session names.
    pub async fn list_sessions(&self) -> Result<Vec<String>, String> {
        if let Some(ref cluster) = self.cluster_node {
            let pairs = cluster.scan_prefix(SL_SESSION_PREFIX)?;
            let names: Vec<String> = pairs
                .into_iter()
                .map(|(k, _)| k[SL_SESSION_PREFIX.len()..].to_string())
                .collect();
            return Ok(names);
        }

        let names: Vec<String> = self
            .db
            .tree_names()
            .into_iter()
            .filter_map(|name| {
                let s = String::from_utf8(name.to_vec()).ok()?;
                if s == "__sled__default" {
                    None
                } else {
                    Some(s)
                }
            })
            .collect();
        Ok(names)
    }

    /// List all entries for a session, optionally filtering to specific fields.
    pub async fn list_entries(
        &self,
        session: &str,
        fields: Option<Vec<String>>,
    ) -> Result<Vec<serde_json::Value>, String> {
        if let Some(ref cluster) = self.cluster_node {
            let prefix = Self::entry_prefix_for_session(session);
            let pairs = cluster.scan_prefix(&prefix)?;
            let mut results = Vec::new();
            for (idx, (_key, val)) in pairs.into_iter().enumerate() {
                let entry: SessionLogEntry = serde_json::from_str(&val)
                    .map_err(|e| format!("Failed to deserialize entry: {}", e))?;
                let value = Self::format_entry(idx as u64, &entry, &fields);
                results.push(value);
            }
            return Ok(results);
        }

        // Standalone mode.
        let tree = self
            .db
            .open_tree(session)
            .map_err(|e| format!("Failed to open tree '{}': {}", session, e))?;

        let mut results = Vec::new();
        for item in tree.iter() {
            let (key_bytes, val_bytes) =
                item.map_err(|e| format!("Failed to read entry: {}", e))?;

            let index = u64::from_be_bytes(
                key_bytes
                    .as_ref()
                    .try_into()
                    .map_err(|_| "Invalid key length".to_string())?,
            );

            let entry: SessionLogEntry = serde_json::from_slice(&val_bytes)
                .map_err(|e| format!("Failed to deserialize entry: {}", e))?;

            let value = Self::format_entry(index, &entry, &fields);
            results.push(value);
        }

        Ok(results)
    }

    /// Get the latest entry for a session, if any.
    pub async fn get_latest(&self, session: &str) -> Result<Option<SessionLogEntry>, String> {
        if let Some(ref cluster) = self.cluster_node {
            let prefix = Self::entry_prefix_for_session(session);
            let pairs = cluster.scan_prefix(&prefix)?;
            return match pairs.last() {
                Some((_key, val)) => {
                    let entry: SessionLogEntry = serde_json::from_str(val)
                        .map_err(|e| format!("Failed to deserialize entry: {}", e))?;
                    Ok(Some(entry))
                }
                None => Ok(None),
            };
        }

        let tree = self
            .db
            .open_tree(session)
            .map_err(|e| format!("Failed to open tree '{}': {}", session, e))?;

        match tree.last() {
            Ok(Some((_key, val))) => {
                let entry: SessionLogEntry = serde_json::from_slice(&val)
                    .map_err(|e| format!("Failed to deserialize entry: {}", e))?;
                Ok(Some(entry))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(format!("Failed to get latest entry: {}", e)),
        }
    }

    /// Seed session `to` from session `from`'s latest snapshot (copy-on-write
    /// session fork). Copies `from`'s most recent entry as `to`'s first entry,
    /// so `to` resumes `from`'s heap+fs on its next execution but writes
    /// subsequent snapshots under its own id — `from` is never modified.
    ///
    /// Idempotent: if `to` already has history, returns [`ForkOutcome::TargetExists`]
    /// without changing anything. If `from` has no history, returns
    /// [`ForkOutcome::SourceEmpty`].
    pub async fn fork(&self, from: &str, to: &str) -> Result<ForkOutcome, String> {
        if self.get_latest(to).await?.is_some() {
            return Ok(ForkOutcome::TargetExists);
        }
        match self.get_latest(from).await? {
            Some(src) => {
                let entry = SessionLogEntry {
                    input_heap: Some(src.output_heap.clone()),
                    output_heap: src.output_heap.clone(),
                    output_fs: src.output_fs.clone(),
                    code: format!("// forked from session {from}"),
                    timestamp: now_iso8601(),
                };
                self.append(to, entry).await?;
                Ok(ForkOutcome::Forked {
                    heap: src.output_heap,
                    fs: src.output_fs,
                })
            }
            None => Ok(ForkOutcome::SourceEmpty),
        }
    }

    /// Flush all pending writes to disk.
    pub fn flush(&self) -> Result<(), String> {
        self.db
            .flush()
            .map_err(|e| format!("Failed to flush sled db: {}", e))?;
        Ok(())
    }

    // --------------------------------------------------------------------
    // Shared formatting helper
    // --------------------------------------------------------------------

    fn format_entry(
        index: u64,
        entry: &SessionLogEntry,
        fields: &Option<Vec<String>>,
    ) -> serde_json::Value {
        match fields {
            Some(field_list) => {
                let mut obj = serde_json::Map::new();
                for field in field_list {
                    match field.as_str() {
                        "index" => {
                            obj.insert(
                                "index".to_string(),
                                serde_json::Value::Number(index.into()),
                            );
                        }
                        "input_heap" => {
                            obj.insert(
                                "input_heap".to_string(),
                                match &entry.input_heap {
                                    Some(h) => serde_json::Value::String(h.clone()),
                                    None => serde_json::Value::Null,
                                },
                            );
                        }
                        "output_heap" => {
                            obj.insert(
                                "output_heap".to_string(),
                                serde_json::Value::String(entry.output_heap.clone()),
                            );
                        }
                        "code" => {
                            obj.insert(
                                "code".to_string(),
                                serde_json::Value::String(entry.code.clone()),
                            );
                        }
                        "timestamp" => {
                            obj.insert(
                                "timestamp".to_string(),
                                serde_json::Value::String(entry.timestamp.clone()),
                            );
                        }
                        _ => {}
                    }
                }
                serde_json::Value::Object(obj)
            }
            None => {
                let mut obj = serde_json::Map::new();
                obj.insert(
                    "index".to_string(),
                    serde_json::Value::Number(index.into()),
                );
                obj.insert(
                    "input_heap".to_string(),
                    match &entry.input_heap {
                        Some(h) => serde_json::Value::String(h.clone()),
                        None => serde_json::Value::Null,
                    },
                );
                obj.insert(
                    "output_heap".to_string(),
                    serde_json::Value::String(entry.output_heap.clone()),
                );
                obj.insert(
                    "code".to_string(),
                    serde_json::Value::String(entry.code.clone()),
                );
                obj.insert(
                    "timestamp".to_string(),
                    serde_json::Value::String(entry.timestamp.clone()),
                );
                serde_json::Value::Object(obj)
            }
        }
    }
}

#[cfg(test)]
mod fork_tests {
    use super::*;
    use tempfile::TempDir;

    fn entry(heap: &str, fs: Option<&str>) -> SessionLogEntry {
        SessionLogEntry {
            input_heap: None,
            output_heap: heap.to_string(),
            output_fs: fs.map(String::from),
            code: "run".to_string(),
            timestamp: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    fn log() -> (SessionLog, TempDir) {
        let dir = TempDir::new().expect("temp dir");
        let log = SessionLog::new(dir.path().to_str().unwrap()).expect("session log");
        (log, dir)
    }

    #[tokio::test]
    async fn fork_seeds_target_from_source_latest_snapshot() {
        let (log, _d) = log();
        log.append("parent", entry("heap1", Some("fs1"))).await.unwrap();
        log.append("parent", entry("heap2", Some("fs2"))).await.unwrap();

        let outcome = log.fork("parent", "child").await.unwrap();
        assert_eq!(
            outcome,
            ForkOutcome::Forked { heap: "heap2".into(), fs: Some("fs2".into()) }
        );
        // The child resumes the parent's LATEST heap+fs.
        let latest = log.get_latest("child").await.unwrap().unwrap();
        assert_eq!(latest.output_heap, "heap2");
        assert_eq!(latest.output_fs, Some("fs2".to_string()));
        assert_eq!(latest.input_heap, Some("heap2".to_string()));
    }

    #[tokio::test]
    async fn fork_is_copy_on_write_source_untouched() {
        let (log, _d) = log();
        log.append("parent", entry("heap1", None)).await.unwrap();
        log.fork("parent", "child").await.unwrap();

        // Advance the child independently.
        log.append("child", entry("heap-child", None)).await.unwrap();

        assert_eq!(log.get_latest("child").await.unwrap().unwrap().output_heap, "heap-child");
        // Parent is unchanged by the child's writes.
        assert_eq!(log.get_latest("parent").await.unwrap().unwrap().output_heap, "heap1");
    }

    #[tokio::test]
    async fn fork_into_existing_session_is_noop() {
        let (log, _d) = log();
        log.append("parent", entry("heap1", None)).await.unwrap();
        log.append("child", entry("existing", None)).await.unwrap();

        let outcome = log.fork("parent", "child").await.unwrap();
        assert_eq!(outcome, ForkOutcome::TargetExists);
        // Existing child history preserved.
        assert_eq!(log.get_latest("child").await.unwrap().unwrap().output_heap, "existing");
    }

    #[tokio::test]
    async fn fork_from_empty_source_is_noop() {
        let (log, _d) = log();
        let outcome = log.fork("nope", "child").await.unwrap();
        assert_eq!(outcome, ForkOutcome::SourceEmpty);
        assert!(log.get_latest("child").await.unwrap().is_none());
    }
}
