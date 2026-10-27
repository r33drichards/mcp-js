use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::cluster::ClusterNode;
use mcp_metadata_store::{MetadataStore, SledStore, Write};

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

/// One session-log entry with its position in the log, for API/FFI/CLI
/// responses. This is the typed shape `list_entries` returns; the MCP tool
/// layer serializes it to JSON (and applies any `fields` projection) itself.
#[derive(Debug, Clone, Serialize, uniffi::Record)]
pub struct SessionSnapshotView {
    pub index: u64,
    pub input_heap: Option<String>,
    pub output_heap: String,
    /// Resulting filesystem snapshot CA id (hex), independent of the heap.
    pub output_fs: Option<String>,
    pub code: String,
    pub timestamp: String,
}

impl SessionSnapshotView {
    fn new(index: u64, entry: SessionLogEntry) -> Self {
        Self {
            index,
            input_heap: entry.input_heap,
            output_heap: entry.output_heap,
            output_fs: entry.output_fs,
            code: entry.code,
            timestamp: entry.timestamp,
        }
    }
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
    /// Standalone metadata backend. Cluster errors never fall back locally.
    db: Arc<dyn MetadataStore>,
    /// When set, writes are routed through Raft consensus and reads come
    /// from the cluster's replicated data tree.
    cluster_node: Option<Arc<ClusterNode>>,
}

impl SessionLog {
    pub fn new(path: &str) -> Result<Self, String> {
        let db = sled::open(path).map_err(|e| format!("Failed to open sled db: {}", e))?;
        Ok(Self {
            db: Arc::new(SledStore(db)),
            cluster_node: None,
        })
    }

    pub fn from_config(config: sled::Config) -> Result<Self, String> {
        let db = config.open().map_err(|e| format!("Failed to open sled db: {}", e))?;
        Ok(Self {
            db: Arc::new(SledStore(db)),
            cluster_node: None,
        })
    }

    /// Use an explicitly selected standalone metadata backend.
    pub fn from_store(db: Arc<dyn MetadataStore>) -> Self {
        Self { db, cluster_node: None }
    }

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

        let value = serde_json::to_vec(&entry).map_err(|e| e.to_string())?;
        let seq = self.db.generate_id().await?;
        self.db.put(Write::new(session, seq.to_be_bytes().to_vec(), value)).await?;
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

        self.db.tree_names().await
    }

    /// List all entries for a session, oldest first.
    pub async fn list_entries(
        &self,
        session: &str,
    ) -> Result<Vec<SessionSnapshotView>, String> {
        if let Some(ref cluster) = self.cluster_node {
            let prefix = Self::entry_prefix_for_session(session);
            let pairs = cluster.scan_prefix(&prefix)?;
            let mut results = Vec::new();
            for (idx, (_key, val)) in pairs.into_iter().enumerate() {
                let entry: SessionLogEntry = serde_json::from_str(&val)
                    .map_err(|e| format!("Failed to deserialize entry: {}", e))?;
                results.push(SessionSnapshotView::new(idx as u64, entry));
            }
            return Ok(results);
        }

        self.db.scan(session, b"", false, None).await?.into_iter().map(|(key, value)| {
            let index = u64::from_be_bytes(key.try_into().map_err(|_| "Invalid key length")?);
            let entry = serde_json::from_slice(&value).map_err(|e| e.to_string())?;
            Ok(SessionSnapshotView::new(index, entry))
        }).collect()
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

        self.db.scan(session, b"", true, Some(1)).await?.first()
            .map(|(_,v)| serde_json::from_slice(v).map_err(|e| e.to_string())).transpose()
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
        self.db.flush()
    }

    // --------------------------------------------------------------------
    // Shared formatting helper
    // --------------------------------------------------------------------

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
