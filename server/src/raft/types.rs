use std::collections::HashMap;
use std::io::Cursor;

use serde::{Deserialize, Serialize};
use openraft::BasicNode;

// Declare the type config for openraft
openraft::declare_raft_types!(
    pub TypeConfig:
        D = Command,
        R = CommandResponse,
        Node = BasicNode,
        SnapshotData = Cursor<Vec<u8>>,
);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Command {
    Execute {
        session_id: String,
        result_ref: SnapshotRef,
    },
    DeleteSession {
        session_id: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandResponse {
    pub session_id: String,
}

/// A pointer into the heap store — NOT the bytes themselves.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotRef {
    /// S3 key or local filesystem path
    pub key: String,
    /// SHA-256 of the heap bytes for integrity verification
    pub sha256: [u8; 32],
}

/// The entire state machine state — intentionally small.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HeapState {
    pub sessions: HashMap<String, SnapshotRef>,
}
