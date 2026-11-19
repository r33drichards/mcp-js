//! Execution registry — tracks in-flight and completed V8 executions.
//!
//! Each execution is assigned a UUID, tracked in an in-memory `DashMap`, and
//! has its console pages indexed by a per-execution runtime tree (`"ex:{id}"`).
//! Server registries store those payloads in the S3/filesystem blob backend.
//! Supports querying console output by line offset or byte offset, and
//! cancellation via V8's `IsolateHandle::terminate_execution()`.

use dashmap::DashMap;
use deno_core::v8;
use mcp_metadata_store::{MetadataStore, RuntimeStore, RuntimeTree};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::artifacts::ArtifactMeta;

// ── Types ────────────────────────────────────────────────────────────────

pub type ExecutionId = String;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
    TimedOut,
}

impl std::fmt::Display for ExecutionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Running => write!(f, "running"),
            Self::Completed => write!(f, "completed"),
            Self::Failed => write!(f, "failed"),
            Self::Cancelled => write!(f, "cancelled"),
            Self::TimedOut => write!(f, "timed_out"),
        }
    }
}

/// In-memory record for an execution.
pub struct ExecutionRecord {
    pub id: ExecutionId,
    pub status: ExecutionStatus,
    pub isolate_handle: Option<v8::IsolateHandle>,
    /// True only after the background task has joined its V8 execution.
    pub stopped: bool,
    pub result: Option<String>,
    pub heap: Option<String>,
    /// Resulting filesystem snapshot CA id (hex), independent of the heap.
    pub fs: Option<String>,
    pub error: Option<String>,
    pub started_at: String,
    pub completed_at: Option<String>,
    /// Artifacts emitted by this execution via `artifact(key, mime, bytes)`.
    pub artifacts: Vec<ArtifactMeta>,
}

/// Summary returned by `list()`.
#[derive(Debug, Clone, Serialize, uniffi::Record)]
pub struct ExecutionSummary {
    #[serde(rename = "execution_id")]
    pub id: ExecutionId,
    pub status: String,
    pub started_at: String,
    pub completed_at: Option<String>,
}

/// A page of console output, including both line and byte coordinates.
#[derive(Debug, Clone, Serialize, uniffi::Record)]
pub struct ConsoleOutputPage {
    pub data: String,

    // Line coordinates
    pub start_line: u64,
    pub end_line: u64,
    pub next_line_offset: u64,
    pub total_lines: u64,

    // Byte coordinates
    pub start_byte: u64,
    pub end_byte: u64,
    pub next_byte_offset: u64,
    pub total_bytes: u64,

    pub has_more: bool,
}

// ── ExecutionRegistry ────────────────────────────────────────────────────

#[derive(Clone)]
pub struct ExecutionRegistry {
    executions: Arc<DashMap<ExecutionId, ExecutionRecord>>,
    db: Option<sled::Db>,
    remote: Option<RuntimeStore>,
    payloads: Option<RuntimeStore>,
    persistence_errors: Arc<DashMap<String, String>>,
}

impl ExecutionRegistry {
    pub fn new(db_path: &str) -> Result<Self, String> {
        let db = sled::open(db_path)
            .map_err(|e| format!("Failed to open execution db at {}: {}", db_path, e))?;
        let backend = Arc::new(super::heap_storage::FileHeapStorage::new(
            std::path::Path::new(db_path).join("runtime-blobs"),
        ));
        let payloads = RuntimeStore::new(Arc::new(mcp_metadata_store::SledStore(db.clone())))?
            .with_blobs(backend);
        Ok(Self {
            executions: Arc::new(DashMap::new()),
            db: Some(db),
            remote: None,
            payloads: Some(payloads),
            persistence_errors: Arc::new(DashMap::new()),
        })
    }

    pub fn from_store(store: Arc<dyn MetadataStore>) -> Result<Self, String> {
        Ok(Self {
            executions: Arc::new(DashMap::new()),
            db: None,
            remote: Some(RuntimeStore::new(store)?),
            payloads: None,
            persistence_errors: Arc::new(DashMap::new()),
        })
    }

    pub fn from_runtime(remote: RuntimeStore) -> Self {
        Self {
            executions: Arc::new(DashMap::new()),
            db: None,
            remote: Some(remote),
            payloads: None,
            persistence_errors: Arc::new(DashMap::new()),
        }
    }

    pub fn with_blobs(
        mut self,
        blobs: Arc<dyn mcp_metadata_store::runtime_tree::RuntimeBlobs>,
    ) -> Self {
        self.remote = self.remote.map(|store| store.with_blobs(blobs.clone()));
        self.payloads = self.payloads.map(|store| store.with_blobs(blobs));
        self
    }

    /// Keep local sled metadata encodings while storing payloads in the selected blob backend.
    pub fn with_local_blob_factory<F>(mut self, factory: F) -> Result<Self, String>
    where
        F: std::future::Future<
                Output = Result<
                    Option<Arc<dyn mcp_metadata_store::runtime_tree::RuntimeBlobs>>,
                    String,
                >,
            > + Send
            + 'static,
    {
        let db = self
            .db
            .clone()
            .ok_or("local execution database unavailable")?;
        self.payloads = Some(RuntimeStore::from_factory(async move {
            let blobs = factory.await?;
            Ok((
                Arc::new(mcp_metadata_store::SledStore(db)) as Arc<dyn MetadataStore>,
                blobs,
            ))
        })?);
        Ok(self)
    }

    fn tree(&self, name: &str) -> Result<RuntimeTree, String> {
        if let Some(store) = &self.remote {
            return Ok(store.tree(name));
        }
        let tree = RuntimeTree::from(
            self.db
                .as_ref()
                .ok_or("execution store unavailable")?
                .open_tree(name)
                .map_err(|e| e.to_string())?,
        );
        Ok(match &self.payloads {
            Some(store) if name == "artifacts" || name.starts_with("ex:") => {
                tree.with_payload_store(store.clone())
            }
            _ => tree,
        })
    }

    fn persist(&self, id: &str) -> Result<(), String> {
        if self.remote.is_none() {
            return Ok(());
        }
        if let Some(info) = self.local_info(id) {
            self.tree("execution_records")?
                .insert(id, serde_json::to_vec(&info).map_err(|e| e.to_string())?)?;
        }
        Ok(())
    }

    /// Register a new execution. Returns its backend tree for console output.
    pub fn register(&self, id: &str) -> Result<RuntimeTree, String> {
        let tree_name = format!("ex:{}", id);
        let tree = self
            .tree(&tree_name)
            .map_err(|e| format!("Failed to open console tree '{}': {}", tree_name, e))?;

        let record = ExecutionRecord {
            id: id.to_string(),
            status: ExecutionStatus::Running,
            isolate_handle: None,
            stopped: false,
            result: None,
            heap: None,
            fs: None,
            error: None,
            started_at: chrono::Utc::now().to_rfc3339(),
            completed_at: None,
            artifacts: Vec::new(),
        };
        self.executions.insert(id.to_string(), record);

        self.persist(id)?;
        Ok(tree)
    }

    /// Open the tree of outstanding artifact upload grants.
    pub fn artifact_uploads_tree(&self) -> Result<RuntimeTree, String> {
        self.tree("artifact_uploads")
            .map_err(|e| format!("Failed to open artifact uploads tree: {}", e))
    }

    /// Open the shared keyed artifact tree (`"artifacts"`) in the execution db.
    pub fn artifacts_tree(&self) -> Result<RuntimeTree, String> {
        self.tree("artifacts")
            .map_err(|e| format!("Failed to open artifacts tree: {}", e))
    }

    /// Record the artifacts an execution emitted via `artifact()`.
    pub fn set_artifacts(&self, id: &str, artifacts: Vec<ArtifactMeta>) {
        if let Some(mut record) = self.executions.get_mut(id) {
            record.artifacts = artifacts;
        }
    }

    /// Store the V8 isolate handle for cancellation support.
    pub fn set_isolate_handle(&self, id: &str, handle: v8::IsolateHandle) {
        if let Some(mut record) = self.executions.get_mut(id) {
            if matches!(record.status, ExecutionStatus::Cancelled) {
                // Cancel can race with startup, before a handle is published.
                handle.terminate_execution();
                return;
            }
            if record.stopped {
                return;
            }
            record.isolate_handle = Some(handle);
        }
    }

    pub fn mark_stopped(&self, id: &str) {
        if let Err(error) = self.persist(id) {
            self.persistence_errors.insert(id.into(), error);
        }
        if let Some(mut record) = self.executions.get_mut(id) {
            record.stopped = true;
            record.isolate_handle = None;
        }
    }

    pub fn is_stopped(&self, id: &str) -> bool {
        self.executions.get(id).is_some_and(|record| record.stopped)
    }

    /// Clear the isolate handle (e.g., after execution completes).
    pub fn clear_isolate_handle(&self, id: &str) {
        if let Some(mut record) = self.executions.get_mut(id) {
            record.isolate_handle = None;
        }
    }

    /// Mark execution as completed with its result.
    pub fn complete(&self, id: &str, output: String, heap: Option<String>) {
        if let Some(mut record) = self.executions.get_mut(id) {
            if !matches!(record.status, ExecutionStatus::Running) {
                return;
            }
            record.status = ExecutionStatus::Completed;
            record.result = Some(output);
            record.heap = heap;
            record.isolate_handle = None;
            record.completed_at = Some(chrono::Utc::now().to_rfc3339());
        }
    }

    /// Record the resulting filesystem snapshot CA id (independent of the heap).
    pub fn set_fs(&self, id: &str, fs: Option<String>) {
        if let Some(mut record) = self.executions.get_mut(id) {
            record.fs = fs;
        }
    }

    /// Mark execution as failed.
    pub fn fail(&self, id: &str, error: String) {
        if let Some(mut record) = self.executions.get_mut(id) {
            if !matches!(record.status, ExecutionStatus::Running) {
                return;
            }
            record.status = ExecutionStatus::Failed;
            record.error = Some(error);
            record.isolate_handle = None;
            record.completed_at = Some(chrono::Utc::now().to_rfc3339());
        }
    }

    /// Mark execution as timed out.
    pub fn timed_out(&self, id: &str) {
        if let Some(mut record) = self.executions.get_mut(id) {
            if !matches!(record.status, ExecutionStatus::Running) {
                return;
            }
            record.status = ExecutionStatus::TimedOut;
            record.error = Some("Execution timed out".to_string());
            record.isolate_handle = None;
            record.completed_at = Some(chrono::Utc::now().to_rfc3339());
        }
    }

    /// Cancel a running execution. Terminates V8 isolate.
    pub fn cancel(&self, id: &str) -> Result<(), String> {
        let mut record = self
            .executions
            .get_mut(id)
            .ok_or_else(|| format!("Execution '{}' not found", id))?;

        match record.status {
            ExecutionStatus::Running => {
                if let Some(ref handle) = record.isolate_handle {
                    handle.terminate_execution();
                }
                record.status = ExecutionStatus::Cancelled;
                record.error = Some("Cancelled by user".to_string());
                record.isolate_handle = None;
                record.completed_at = Some(chrono::Utc::now().to_rfc3339());
                Ok(())
            }
            _ => Err(format!(
                "Execution '{}' is not running (status: {})",
                id, record.status
            )),
        }
    }

    /// Cancel every currently running execution. Returns the number cancelled.
    pub fn cancel_all(&self) -> u64 {
        let running: Vec<ExecutionId> = self
            .executions
            .iter()
            .filter(|entry| matches!(entry.value().status, ExecutionStatus::Running))
            .map(|entry| entry.key().clone())
            .collect();
        running.iter().filter(|id| self.cancel(id).is_ok()).count() as u64
    }

    /// Get execution status and result.
    fn local_info(&self, id: &str) -> Option<ExecutionInfo> {
        self.executions.get(id).map(|r| ExecutionInfo {
            id: r.id.clone(),
            status: r.status.to_string(),
            result: r.result.clone(),
            heap: r.heap.clone(),
            fs: r.fs.clone(),
            error: r.error.clone(),
            started_at: r.started_at.clone(),
            completed_at: r.completed_at.clone(),
            artifacts: r.artifacts.clone(),
        })
    }

    pub fn get(&self, id: &str) -> Result<Option<ExecutionInfo>, String> {
        if let Some(error) = self.persistence_errors.get(id) {
            return Err(format!(
                "execution metadata persistence failed: {}",
                error.value()
            ));
        }
        if let Some(info) = self.local_info(id) {
            return Ok(Some(info));
        }
        if self.remote.is_none() {
            return Ok(None);
        }
        self.tree("execution_records")?
            .get(id)?
            .map(|v| serde_json::from_slice(&v).map_err(|e| e.to_string()))
            .transpose()
    }

    /// List all executions.
    pub fn list(&self) -> Result<Vec<ExecutionSummary>, String> {
        let mut entries = std::collections::BTreeMap::new();
        if self.remote.is_some() {
            for row in self.tree("execution_records")?.iter() {
                let (_, value) = row?;
                let info: ExecutionInfo =
                    serde_json::from_slice(&value).map_err(|e| e.to_string())?;
                entries.insert(
                    info.id.clone(),
                    ExecutionSummary {
                        id: info.id,
                        status: info.status,
                        started_at: info.started_at,
                        completed_at: info.completed_at,
                    },
                );
            }
        }
        for entry in self.executions.iter() {
            let r = entry.value();
            entries.insert(
                r.id.clone(),
                ExecutionSummary {
                    id: r.id.clone(),
                    status: r.status.to_string(),
                    started_at: r.started_at.clone(),
                    completed_at: r.completed_at.clone(),
                },
            );
        }
        Ok(entries.into_values().collect())
    }

    /// Reclaim unreachable managed local objects. Every registry using the same
    /// directory must participate; active readers/writers make maintenance busy.
    pub fn collect_runtime_objects(
        registries: &[&Self],
        minimum_age: std::time::Duration,
    ) -> Result<usize, String> {
        let stores = registries
            .iter()
            .map(|r| {
                r.remote
                    .as_ref()
                    .or(r.payloads.as_ref())
                    .cloned()
                    .ok_or("runtime store unavailable".into())
            })
            .collect::<Result<Vec<_>, String>>()?;
        RuntimeStore::collect_runtime_objects(&stores, minimum_age)
    }

    /// Get the execution status string for a given ID.
    pub fn get_status(&self, id: &str) -> Option<String> {
        self.executions.get(id).map(|r| r.status.to_string())
    }

    /// Query console output with dual mode: line-based or byte-based offsets.
    /// If `byte_offset` is provided, byte-offset mode is used (takes precedence).
    /// Otherwise, line-offset mode is used.
    pub fn get_console_output(
        &self,
        id: &str,
        line_offset: Option<u64>,
        line_limit: Option<u64>,
        byte_offset: Option<u64>,
        byte_limit: Option<u64>,
    ) -> Result<ConsoleOutputPage, String> {
        let tree_name = format!("ex:{}", id);
        let tree = self
            .tree(&tree_name)
            .map_err(|e| format!("Failed to open console tree: {}", e))?;

        let pages = tree.pages()?;
        let total_bytes = pages.iter().try_fold(0u64, |n, p| {
            n.checked_add(p.length).ok_or("console length overflow")
        })?;
        let total_lines = pages.iter().try_fold(0u64, |n, p| {
            n.checked_add(p.newlines)
                .ok_or("console line count overflow")
        })?;
        if let Some(byte_offset) = byte_offset {
            let start = byte_offset.min(total_bytes);
            let end = start
                .saturating_add(byte_limit.unwrap_or(4096))
                .min(total_bytes);
            let mut offset = 0u64;
            let mut previous_lines = 0u64;
            let mut start_line = total_lines + 1;
            let mut end_line = start_line;
            let mut output = Vec::new();
            let mut found = false;
            for page in &pages {
                let page_end = offset + page.length;
                if page_end <= start {
                    previous_lines += page.newlines;
                    offset = page_end;
                    continue;
                }
                if offset >= end && (found || start != end) {
                    break;
                }
                let bytes = page.read()?;
                let left = start.saturating_sub(offset).min(page.length) as usize;
                let right = end.saturating_sub(offset).min(page.length) as usize;
                if !found {
                    start_line = previous_lines
                        + bytes[..left].iter().filter(|&&b| b == b'\n').count() as u64
                        + 1;
                    found = true;
                }
                end_line = previous_lines
                    + bytes[..right].iter().filter(|&&b| b == b'\n').count() as u64
                    + 1;
                output.extend_from_slice(&bytes[left..right]);
                previous_lines += page.newlines;
                offset = page_end;
                if page_end >= end {
                    break;
                }
            }
            return Ok(ConsoleOutputPage {
                data: String::from_utf8_lossy(&output).into_owned(),
                start_line,
                end_line,
                next_line_offset: end_line,
                total_lines,
                start_byte: start,
                end_byte: end,
                next_byte_offset: end,
                total_bytes,
                has_more: end < total_bytes,
            });
        }
        let line_offset = line_offset.unwrap_or(1);
        let skip = line_offset.saturating_sub(1);
        let limit = line_limit.unwrap_or(100);
        let unterminated = pages
            .iter()
            .rev()
            .find(|p| p.length > 0)
            .is_some_and(|p| !p.ends_with_newline);
        let logical_lines = total_lines + u64::from(unterminated);
        if skip >= logical_lines {
            return Ok(ConsoleOutputPage {
                data: String::new(),
                start_line: 0,
                end_line: 0,
                next_line_offset: 1,
                total_lines,
                start_byte: total_bytes,
                end_byte: total_bytes,
                next_byte_offset: total_bytes,
                total_bytes,
                has_more: false,
            });
        }
        let mut remaining = skip;
        let mut offset = 0u64;
        let mut start_byte = None;
        let mut end_byte = 0u64;
        let mut output = Vec::new();
        let mut selected_lines = 0u64;
        for page in &pages {
            if start_byte.is_none()
                && (remaining > page.newlines
                    || (remaining > 0 && remaining == page.newlines && page.ends_with_newline))
            {
                remaining -= page.newlines;
                offset += page.length;
                continue;
            }
            let bytes = page.read()?;
            let mut local = 0;
            if start_byte.is_none() {
                while remaining > 0 {
                    let next = bytes[local..]
                        .iter()
                        .position(|&b| b == b'\n')
                        .ok_or("invalid console newline index")?;
                    local += next + 1;
                    remaining -= 1;
                }
                start_byte = Some(offset + local as u64);
                end_byte = offset + local as u64;
            }
            if limit == 0 {
                break;
            }
            while local < bytes.len() {
                let byte = bytes[local];
                output.push(byte);
                local += 1;
                end_byte = offset + local as u64;
                if byte == b'\n' {
                    selected_lines += 1;
                    if selected_lines == limit {
                        break;
                    }
                }
            }
            if selected_lines == limit {
                break;
            }
            offset += page.length;
        }
        if !output.is_empty() && output.last() != Some(&b'\n') {
            selected_lines += 1;
        }
        if output.last() == Some(&b'\n') {
            output.pop();
        }
        let start_line = if selected_lines == 0 { 0 } else { line_offset };
        let end_line = if selected_lines == 0 {
            0
        } else {
            start_line + selected_lines - 1
        };
        let start_byte = start_byte.unwrap_or(total_bytes);
        Ok(ConsoleOutputPage {
            data: String::from_utf8_lossy(&output).into_owned(),
            start_line,
            end_line,
            next_line_offset: if selected_lines == 0 { 1 } else { end_line + 1 },
            total_lines,
            start_byte,
            end_byte,
            next_byte_offset: end_byte,
            total_bytes,
            has_more: skip + selected_lines < logical_lines,
        })
    }
}

/// Execution info returned to callers.
#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
pub struct ExecutionInfo {
    pub id: ExecutionId,
    pub status: String,
    pub result: Option<String>,
    pub heap: Option<String>,
    /// Resulting filesystem snapshot CA id (hex), independent of the heap.
    pub fs: Option<String>,
    pub error: Option<String>,
    pub started_at: String,
    pub completed_at: Option<String>,
    /// Artifacts emitted by this execution via `artifact(key, mime, bytes)`.
    pub artifacts: Vec<ArtifactMeta>,
}

#[cfg(test)]
mod tests {
    use super::ExecutionRegistry;

    #[test]
    fn cancel_all_only_cancels_running_executions() {
        let dir = tempfile::tempdir().unwrap();
        let registry = ExecutionRegistry::new(dir.path().to_str().unwrap()).unwrap();
        registry.register("running").unwrap();
        registry.register("completed").unwrap();
        registry.complete("completed", "done".to_string(), None);

        assert_eq!(registry.cancel_all(), 1);
        assert_eq!(registry.get_status("running").as_deref(), Some("cancelled"));
        assert_eq!(
            registry.get_status("completed").as_deref(),
            Some("completed")
        );
        registry.fail("running", "late failure".to_string());
        registry.complete("running", "late completion".to_string(), None);
        assert_eq!(registry.get_status("running").as_deref(), Some("cancelled"));
        assert_eq!(registry.cancel_all(), 0);
    }
}

#[cfg(test)]
mod persistence_tests {
    use super::*;
    #[test]
    fn durable_execution_and_console_recover_without_local_registry() {
        let db = sled::Config::new().temporary(true).open().unwrap();
        let store = Arc::new(mcp_metadata_store::SledStore(db));
        let first = ExecutionRegistry::from_store(store.clone()).unwrap();
        let console = first.register("durable").unwrap();
        console
            .insert(0u64.to_be_bytes(), b"persisted output\n")
            .unwrap();
        first.complete("durable", "42".into(), None);
        first.set_fs("durable", Some("snapshot".into()));
        first.mark_stopped("durable");
        drop(first);
        let second = ExecutionRegistry::from_store(store).unwrap();
        let info = second.get("durable").unwrap().unwrap();
        assert_eq!(info.status, "completed");
        assert_eq!(info.result.as_deref(), Some("42"));
        assert_eq!(info.fs.as_deref(), Some("snapshot"));
        assert_eq!(second.list().unwrap().len(), 1);
        assert_eq!(
            second
                .get_console_output("durable", None, None, Some(0), Some(100))
                .unwrap()
                .data,
            "persisted output\n"
        );
        assert!(second.cancel("durable").is_err()); // A persisted record is not a local isolate.
    }
}
