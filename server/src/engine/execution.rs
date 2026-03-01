//! Execution registry — tracks in-flight and completed V8 executions.
//!
//! Each execution is assigned a UUID, tracked in an in-memory `DashMap`, and
//! has its console output stored in a per-execution sled tree (`"ex:{id}"`).
//! Supports querying console output by line offset or byte offset, and
//! cancellation via V8's `IsolateHandle::terminate_execution()`.

use std::sync::Arc;
use dashmap::DashMap;
use deno_core::v8;
use serde::Serialize;

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
    pub result: Option<String>,
    pub heap: Option<String>,
    pub error: Option<String>,
    pub started_at: String,
    pub completed_at: Option<String>,
}

/// Summary returned by `list()`.
#[derive(Debug, Clone, Serialize)]
pub struct ExecutionSummary {
    pub id: ExecutionId,
    pub status: String,
    pub started_at: String,
    pub completed_at: Option<String>,
}

/// A page of console output, including both line and byte coordinates.
#[derive(Debug, Clone, Serialize)]
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
    db: sled::Db,
}

impl ExecutionRegistry {
    pub fn new(db_path: &str) -> Result<Self, String> {
        let db = sled::open(db_path)
            .map_err(|e| format!("Failed to open execution db at {}: {}", db_path, e))?;
        Ok(Self {
            executions: Arc::new(DashMap::new()),
            db,
        })
    }

    /// Register a new execution. Returns the sled tree for console output.
    pub fn register(&self, id: &str) -> Result<sled::Tree, String> {
        let tree_name = format!("ex:{}", id);
        let tree = self.db.open_tree(&tree_name)
            .map_err(|e| format!("Failed to open console tree '{}': {}", tree_name, e))?;

        let record = ExecutionRecord {
            id: id.to_string(),
            status: ExecutionStatus::Running,
            isolate_handle: None,
            result: None,
            heap: None,
            error: None,
            started_at: chrono::Utc::now().to_rfc3339(),
            completed_at: None,
        };
        self.executions.insert(id.to_string(), record);

        Ok(tree)
    }

    /// Store the V8 isolate handle for cancellation support.
    pub fn set_isolate_handle(&self, id: &str, handle: v8::IsolateHandle) {
        if let Some(mut record) = self.executions.get_mut(id) {
            record.isolate_handle = Some(handle);
        }
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
            record.status = ExecutionStatus::Completed;
            record.result = Some(output);
            record.heap = heap;
            record.isolate_handle = None;
            record.completed_at = Some(chrono::Utc::now().to_rfc3339());
        }
    }

    /// Mark execution as failed.
    pub fn fail(&self, id: &str, error: String) {
        if let Some(mut record) = self.executions.get_mut(id) {
            record.status = ExecutionStatus::Failed;
            record.error = Some(error);
            record.isolate_handle = None;
            record.completed_at = Some(chrono::Utc::now().to_rfc3339());
        }
    }

    /// Mark execution as timed out.
    pub fn timed_out(&self, id: &str) {
        if let Some(mut record) = self.executions.get_mut(id) {
            record.status = ExecutionStatus::TimedOut;
            record.error = Some("Execution timed out".to_string());
            record.isolate_handle = None;
            record.completed_at = Some(chrono::Utc::now().to_rfc3339());
        }
    }

    /// Cancel a running execution. Terminates V8 isolate.
    pub fn cancel(&self, id: &str) -> Result<(), String> {
        let mut record = self.executions.get_mut(id)
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

    /// Get execution status and result.
    pub fn get(&self, id: &str) -> Option<ExecutionInfo> {
        self.executions.get(id).map(|r| ExecutionInfo {
            id: r.id.clone(),
            status: r.status.to_string(),
            result: r.result.clone(),
            heap: r.heap.clone(),
            error: r.error.clone(),
            started_at: r.started_at.clone(),
            completed_at: r.completed_at.clone(),
        })
    }

    /// List all executions.
    pub fn list(&self) -> Vec<ExecutionSummary> {
        self.executions.iter().map(|entry| {
            let r = entry.value();
            ExecutionSummary {
                id: r.id.clone(),
                status: r.status.to_string(),
                started_at: r.started_at.clone(),
                completed_at: r.completed_at.clone(),
            }
        }).collect()
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
        let tree = self.db.open_tree(&tree_name)
            .map_err(|e| format!("Failed to open console tree: {}", e))?;

        // Reconstruct full byte stream from WAL pages
        let mut full_output = Vec::new();
        for item in tree.iter() {
            let (_, value) = item.map_err(|e| format!("Failed to read console entry: {}", e))?;
            full_output.extend_from_slice(&value);
        }
        let total_bytes = full_output.len() as u64;
        let full_str = String::from_utf8_lossy(&full_output);
        let total_lines = full_str.matches('\n').count() as u64;

        if let Some(byte_off) = byte_offset {
            // === Byte-offset mode ===
            let limit = byte_limit.unwrap_or(4096);
            let start = (byte_off as usize).min(full_output.len());
            let end = (start + limit as usize).min(full_output.len());
            let data = String::from_utf8_lossy(&full_output[start..end]).to_string();
            let start_line = full_output[..start].iter().filter(|&&b| b == b'\n').count() as u64 + 1;
            let end_line = full_output[..end].iter().filter(|&&b| b == b'\n').count() as u64 + 1;

            Ok(ConsoleOutputPage {
                data,
                start_line,
                end_line,
                next_line_offset: end_line,
                total_lines,
                start_byte: start as u64,
                end_byte: end as u64,
                next_byte_offset: end as u64,
                total_bytes,
                has_more: end < full_output.len(),
            })
        } else {
            // === Line-offset mode (default) ===
            let line_off = line_offset.unwrap_or(1);
            let limit = line_limit.unwrap_or(100);

            let lines: Vec<&str> = full_str.split('\n').collect();
            let lines = if lines.last() == Some(&"") {
                &lines[..lines.len() - 1]
            } else {
                &lines[..]
            };

            let start_idx = (line_off.saturating_sub(1)) as usize;
            let page: Vec<&str> = lines.iter()
                .skip(start_idx)
                .take(limit as usize)
                .copied()
                .collect();

            // Compute byte range for the selected lines
            let start_byte: u64 = if start_idx < lines.len() {
                lines[..start_idx].iter().map(|l| l.len() as u64 + 1).sum()
            } else {
                total_bytes
            };
            let page_bytes: u64 = page.iter().map(|l| l.len() as u64 + 1).sum();

            let data = page.join("\n");
            let start_line = if page.is_empty() { 0 } else { line_off };
            let end_line = if page.is_empty() {
                0
            } else {
                start_line + page.len() as u64 - 1
            };

            Ok(ConsoleOutputPage {
                data,
                start_line,
                end_line,
                next_line_offset: if end_line > 0 { end_line + 1 } else { 1 },
                total_lines,
                start_byte,
                end_byte: start_byte + page_bytes,
                next_byte_offset: start_byte + page_bytes,
                total_bytes,
                has_more: (start_idx + page.len()) < lines.len(),
            })
        }
    }
}

/// Execution info returned to callers.
#[derive(Debug, Clone, Serialize)]
pub struct ExecutionInfo {
    pub id: ExecutionId,
    pub status: String,
    pub result: Option<String>,
    pub heap: Option<String>,
    pub error: Option<String>,
    pub started_at: String,
    pub completed_at: Option<String>,
}
