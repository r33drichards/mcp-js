//! The mutable pointer plane: human-readable labels mapping a name to a current
//! manifest CA id, plus a per-label append-only reflog enabling rollback.
//!
//! This is the only coordinated write in the system — blobs and manifests are
//! immutable and idempotent. In single-node mode CAS is serialized by an
//! in-process mutex around read-compare-write; cluster forwarding is wired in
//! a later task. Persistence mirrors `heap_tags.rs` (a sled DB).

use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub type CaId = [u8; 32];

const HEAD_TREE: &str = "fs_label_heads";
const LOG_TREE: &str = "fs_label_log";

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum RefOp {
    Create,
    Push,
    Reset,
    Force,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RefLogEntry {
    /// Unix timestamp (millis) when the move happened.
    pub at: i64,
    pub from: Option<CaId>,
    pub to: CaId,
    pub op: RefOp,
}

#[derive(Clone)]
pub struct LabelStore {
    db: sled::Db,
    /// Serializes read-compare-write across in-process callers so CAS is atomic.
    lock: Arc<tokio::sync::Mutex<()>>,
}

impl LabelStore {
    pub fn new(path: &str) -> Result<Self, String> {
        let db = sled::open(path).map_err(|e| format!("Failed to open sled db: {e}"))?;
        Ok(Self {
            db,
            lock: Arc::new(tokio::sync::Mutex::new(())),
        })
    }

    pub fn from_db(db: sled::Db) -> Self {
        Self {
            db,
            lock: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    /// Ephemeral in-memory store. Available in normal builds so integration-test
    /// crates can construct one.
    pub fn in_memory() -> Self {
        let db = sled::Config::new()
            .temporary(true)
            .open()
            .expect("open temporary sled db");
        Self::from_db(db)
    }

    fn heads(&self) -> Result<sled::Tree, String> {
        self.db
            .open_tree(HEAD_TREE)
            .map_err(|e| format!("open heads tree: {e}"))
    }

    fn logs(&self) -> Result<sled::Tree, String> {
        self.db
            .open_tree(LOG_TREE)
            .map_err(|e| format!("open log tree: {e}"))
    }

    fn read_head(&self, label: &str) -> Result<Option<CaId>, String> {
        let heads = self.heads()?;
        match heads.get(label.as_bytes()).map_err(|e| e.to_string())? {
            Some(v) => to_caid(&v).map(Some),
            None => Ok(None),
        }
    }

    /// Append a reflog entry. Keyed by a globally monotonic id so entries scan
    /// back in insertion order within a label's prefix.
    fn append_log(&self, label: &str, entry: &RefLogEntry) -> Result<(), String> {
        let logs = self.logs()?;
        let id = self.db.generate_id().map_err(|e| e.to_string())?;
        let key = log_key(label, id);
        let val = serde_json::to_vec(entry).map_err(|e| e.to_string())?;
        logs.insert(key, val).map_err(|e| e.to_string())?;
        Ok(())
    }

    fn now() -> i64 {
        chrono::Utc::now().timestamp_millis()
    }

    /// Create a new label pointing at `head`. Fails if it already exists.
    pub async fn create(&self, label: &str, head: CaId) -> Result<(), String> {
        let _guard = self.lock.lock().await;
        if self.read_head(label)?.is_some() {
            return Err(format!("label already exists: {label}"));
        }
        self.heads()?
            .insert(label.as_bytes(), &head)
            .map_err(|e| e.to_string())?;
        self.append_log(
            label,
            &RefLogEntry {
                at: Self::now(),
                from: None,
                to: head,
                op: RefOp::Create,
            },
        )?;
        Ok(())
    }

    pub async fn resolve(&self, label: &str) -> Result<Option<CaId>, String> {
        self.read_head(label)
    }

    /// Atomic compare-and-set. Advances `label` to `new` only if its current
    /// head equals `expect`; records a `Push` reflog entry on success.
    pub async fn cas(&self, label: &str, expect: Option<CaId>, new: CaId) -> Result<bool, String> {
        let _guard = self.lock.lock().await;
        let current = self.read_head(label)?;
        if current != expect {
            return Ok(false);
        }
        self.heads()?
            .insert(label.as_bytes(), &new)
            .map_err(|e| e.to_string())?;
        self.append_log(
            label,
            &RefLogEntry {
                at: Self::now(),
                from: current,
                to: new,
                op: RefOp::Push,
            },
        )?;
        Ok(true)
    }

    /// Unconditionally move `label` to `target` (the `reset` verb). Records a
    /// `Reset` reflog entry, retaining the rolled-past id for roll-forward/GC.
    pub async fn force(&self, label: &str, target: CaId) -> Result<(), String> {
        let _guard = self.lock.lock().await;
        let current = self.read_head(label)?;
        self.heads()?
            .insert(label.as_bytes(), &target)
            .map_err(|e| e.to_string())?;
        self.append_log(
            label,
            &RefLogEntry {
                at: Self::now(),
                from: current,
                to: target,
                op: RefOp::Reset,
            },
        )?;
        Ok(())
    }

    pub async fn log(&self, label: &str) -> Result<Vec<RefLogEntry>, String> {
        let logs = self.logs()?;
        let prefix = log_prefix(label);
        let mut out = Vec::new();
        for item in logs.scan_prefix(&prefix) {
            let (_k, v) = item.map_err(|e| e.to_string())?;
            let entry: RefLogEntry = serde_json::from_slice(&v).map_err(|e| e.to_string())?;
            out.push(entry);
        }
        Ok(out)
    }

    pub async fn list(&self) -> Result<Vec<(String, CaId)>, String> {
        let heads = self.heads()?;
        let mut out = Vec::new();
        for item in heads.iter() {
            let (k, v) = item.map_err(|e| e.to_string())?;
            let label = String::from_utf8_lossy(&k).to_string();
            out.push((label, to_caid(&v)?));
        }
        Ok(out)
    }
}

fn to_caid(bytes: &[u8]) -> Result<CaId, String> {
    bytes
        .try_into()
        .map_err(|_| format!("invalid CA id length: {}", bytes.len()))
}

/// Log keys are `{label}\0{id:020}` so a `scan_prefix` over `{label}\0` yields
/// exactly that label's entries in insertion order. A NUL separator keeps a
/// label name from being a prefix of another (e.g. "main" vs "main2").
fn log_prefix(label: &str) -> Vec<u8> {
    let mut p = label.as_bytes().to_vec();
    p.push(0);
    p
}

fn log_key(label: &str, id: u64) -> Vec<u8> {
    let mut k = log_prefix(label);
    k.extend_from_slice(format!("{id:020}").as_bytes());
    k
}
