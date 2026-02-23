use std::fmt::Debug;
use std::io::Cursor;
use std::ops::RangeBounds;
use std::sync::Arc;

use openraft::storage::RaftLogReader;
use openraft::storage::RaftSnapshotBuilder;
use openraft::storage::RaftStorage;
use openraft::BasicNode;
use openraft::Entry;
use openraft::EntryPayload;
use openraft::LogId;
use openraft::LogState;
use openraft::OptionalSend;
use openraft::RaftLogId;
use openraft::Snapshot;
use openraft::SnapshotMeta;
use openraft::StorageError;
use openraft::StorageIOError;
use openraft::StoredMembership;
use openraft::Vote;

use tokio::sync::RwLock;

use crate::raft::types::{Command, CommandResponse, HeapState, TypeConfig};

// --- Snapshot stored in memory ---

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StoredSnapshot {
    pub meta: SnapshotMeta<u64, BasicNode>,
    pub data: Vec<u8>,
}

// --- State machine data ---

#[derive(Debug)]
pub struct StateMachineData {
    pub last_applied_log: Option<LogId<u64>>,
    pub last_membership: StoredMembership<u64, BasicNode>,
    pub state: HeapState,
}

impl Default for StateMachineData {
    fn default() -> Self {
        Self {
            last_applied_log: None,
            last_membership: StoredMembership::default(),
            state: HeapState::default(),
        }
    }
}

// --- The combined store ---

#[derive(Clone)]
pub struct SledStore {
    #[allow(dead_code)]
    db: sled::Db,
    log_tree: sled::Tree,
    meta_tree: sled::Tree,
    pub sm: Arc<RwLock<StateMachineData>>,
    snapshot: Arc<RwLock<Option<StoredSnapshot>>>,
}

impl SledStore {
    pub fn new(db: sled::Db) -> Result<Self, sled::Error> {
        let log_tree = db.open_tree("log")?;
        let meta_tree = db.open_tree("meta")?;
        Ok(Self {
            db,
            log_tree,
            meta_tree,
            sm: Arc::new(RwLock::new(StateMachineData::default())),
            snapshot: Arc::new(RwLock::new(None)),
        })
    }

    /// Get the current committed heap state for reads.
    pub async fn get_state(&self) -> HeapState {
        let data = self.sm.read().await;
        data.state.clone()
    }

    fn index_to_key(index: u64) -> [u8; 8] {
        index.to_be_bytes()
    }

    fn get_meta<T: serde::de::DeserializeOwned>(&self, key: &str) -> Result<Option<T>, StorageError<u64>> {
        let val = self.meta_tree.get(key).map_err(|e| {
            StorageIOError::read(&e)
        })?;
        match val {
            Some(bytes) => {
                let v: T = serde_json::from_slice(&bytes).map_err(|e| {
                    StorageIOError::read(&e)
                })?;
                Ok(Some(v))
            }
            None => Ok(None),
        }
    }

    fn set_meta<T: serde::Serialize>(&self, key: &str, value: &T) -> Result<(), StorageError<u64>> {
        let bytes = serde_json::to_vec(value).map_err(|e| {
            StorageIOError::write(&e)
        })?;
        self.meta_tree.insert(key, bytes).map_err(|e| {
            StorageIOError::write(&e)
        })?;
        Ok(())
    }
}

// --- RaftLogReader ---

impl RaftLogReader<TypeConfig> for SledStore {
    async fn try_get_log_entries<RB: RangeBounds<u64> + Clone + Debug + OptionalSend>(
        &mut self,
        range: RB,
    ) -> Result<Vec<Entry<TypeConfig>>, StorageError<u64>> {
        let start = match range.start_bound() {
            std::ops::Bound::Included(&v) => v,
            std::ops::Bound::Excluded(&v) => v + 1,
            std::ops::Bound::Unbounded => 0,
        };
        let end = match range.end_bound() {
            std::ops::Bound::Included(&v) => v + 1,
            std::ops::Bound::Excluded(&v) => v,
            std::ops::Bound::Unbounded => u64::MAX,
        };

        let mut entries = Vec::new();
        let start_key = Self::index_to_key(start);
        let end_key = Self::index_to_key(end);

        for item in self.log_tree.range(start_key..end_key) {
            let (_key, value) = item.map_err(|e| StorageIOError::read(&e))?;
            let entry: Entry<TypeConfig> =
                serde_json::from_slice(&value).map_err(|e| StorageIOError::read(&e))?;
            entries.push(entry);
        }

        Ok(entries)
    }
}

// --- RaftSnapshotBuilder ---

impl RaftSnapshotBuilder<TypeConfig> for SledStore {
    async fn build_snapshot(&mut self) -> Result<Snapshot<TypeConfig>, StorageError<u64>> {
        let sm = self.sm.read().await;

        let state_bytes = serde_json::to_vec(&sm.state).map_err(|e| StorageIOError::write(&e))?;

        let last_applied = sm.last_applied_log;
        let last_membership = sm.last_membership.clone();

        let snapshot_id = format!(
            "{}-{}",
            last_applied.map(|l| l.to_string()).unwrap_or_default(),
            "0"
        );

        let meta = SnapshotMeta {
            last_log_id: last_applied,
            last_membership,
            snapshot_id,
        };

        let snapshot = StoredSnapshot {
            meta: meta.clone(),
            data: state_bytes.clone(),
        };

        {
            let mut current_snapshot = self.snapshot.write().await;
            *current_snapshot = Some(snapshot);
        }

        Ok(Snapshot {
            meta,
            snapshot: Box::new(Cursor::new(state_bytes)),
        })
    }
}

// --- RaftStorage (V1 unified trait) ---

impl RaftStorage<TypeConfig> for SledStore {
    type LogReader = Self;
    type SnapshotBuilder = Self;

    // --- Vote ---

    async fn save_vote(&mut self, vote: &Vote<u64>) -> Result<(), StorageError<u64>> {
        self.set_meta("vote", vote)?;
        self.meta_tree.flush_async().await.map_err(|e| StorageIOError::write(&e))?;
        Ok(())
    }

    async fn read_vote(&mut self) -> Result<Option<Vote<u64>>, StorageError<u64>> {
        self.get_meta("vote")
    }

    async fn save_committed(&mut self, committed: Option<LogId<u64>>) -> Result<(), StorageError<u64>> {
        self.set_meta("committed", &committed)?;
        Ok(())
    }

    async fn read_committed(&mut self) -> Result<Option<LogId<u64>>, StorageError<u64>> {
        self.get_meta("committed")
    }

    // --- Log ---

    async fn get_log_state(&mut self) -> Result<LogState<TypeConfig>, StorageError<u64>> {
        let last_purged: Option<LogId<u64>> = self.get_meta("last_purged")?;

        let last_log_id = self
            .log_tree
            .last()
            .map_err(|e| StorageIOError::read(&e))?
            .map(|(_k, v)| {
                let entry: Entry<TypeConfig> = serde_json::from_slice(&v).unwrap();
                *entry.get_log_id()
            });

        let last_log_id = last_log_id.or(last_purged);

        Ok(LogState {
            last_purged_log_id: last_purged,
            last_log_id,
        })
    }

    async fn get_log_reader(&mut self) -> Self::LogReader {
        self.clone()
    }

    async fn append_to_log<I>(&mut self, entries: I) -> Result<(), StorageError<u64>>
    where
        I: IntoIterator<Item = Entry<TypeConfig>> + OptionalSend,
    {
        for entry in entries {
            let index = entry.get_log_id().index;
            let key = Self::index_to_key(index);
            let value = serde_json::to_vec(&entry).map_err(|e| StorageIOError::write(&e))?;
            self.log_tree.insert(key, value).map_err(|e| StorageIOError::write(&e))?;
        }
        self.log_tree.flush_async().await.map_err(|e| StorageIOError::write(&e))?;
        Ok(())
    }

    async fn delete_conflict_logs_since(&mut self, log_id: LogId<u64>) -> Result<(), StorageError<u64>> {
        let start_key = Self::index_to_key(log_id.index);
        let keys_to_remove: Vec<_> = self
            .log_tree
            .range(start_key..)
            .filter_map(|item| item.ok().map(|(k, _v)| k))
            .collect();
        for key in keys_to_remove {
            self.log_tree.remove(key).map_err(|e| StorageIOError::write(&e))?;
        }
        Ok(())
    }

    async fn purge_logs_upto(&mut self, log_id: LogId<u64>) -> Result<(), StorageError<u64>> {
        let end_key = Self::index_to_key(log_id.index + 1);
        let keys_to_remove: Vec<_> = self
            .log_tree
            .range(..end_key)
            .filter_map(|item| item.ok().map(|(k, _v)| k))
            .collect();
        for key in keys_to_remove {
            self.log_tree.remove(key).map_err(|e| StorageIOError::write(&e))?;
        }
        self.set_meta("last_purged", &log_id)?;
        Ok(())
    }

    // --- State Machine ---

    async fn last_applied_state(
        &mut self,
    ) -> Result<(Option<LogId<u64>>, StoredMembership<u64, BasicNode>), StorageError<u64>> {
        let sm = self.sm.read().await;
        Ok((sm.last_applied_log, sm.last_membership.clone()))
    }

    async fn apply_to_state_machine(&mut self, entries: &[Entry<TypeConfig>]) -> Result<Vec<CommandResponse>, StorageError<u64>> {
        let mut responses = Vec::new();
        let mut sm = self.sm.write().await;

        for entry in entries {
            sm.last_applied_log = Some(*entry.get_log_id());

            match &entry.payload {
                EntryPayload::Blank => {
                    responses.push(CommandResponse {
                        session_id: String::new(),
                    });
                }
                EntryPayload::Normal(cmd) => match cmd {
                    Command::Execute {
                        session_id,
                        result_ref,
                    } => {
                        sm.state.sessions.insert(session_id.clone(), result_ref.clone());
                        responses.push(CommandResponse {
                            session_id: session_id.clone(),
                        });
                    }
                    Command::DeleteSession { session_id } => {
                        sm.state.sessions.remove(session_id);
                        responses.push(CommandResponse {
                            session_id: session_id.clone(),
                        });
                    }
                },
                EntryPayload::Membership(mem) => {
                    sm.last_membership = StoredMembership::new(Some(*entry.get_log_id()), mem.clone());
                    responses.push(CommandResponse {
                        session_id: String::new(),
                    });
                }
            }
        }

        Ok(responses)
    }

    // --- Snapshot ---

    async fn get_snapshot_builder(&mut self) -> Self::SnapshotBuilder {
        self.clone()
    }

    async fn begin_receiving_snapshot(&mut self) -> Result<Box<Cursor<Vec<u8>>>, StorageError<u64>> {
        Ok(Box::new(Cursor::new(Vec::new())))
    }

    async fn install_snapshot(
        &mut self,
        meta: &SnapshotMeta<u64, BasicNode>,
        snapshot: Box<Cursor<Vec<u8>>>,
    ) -> Result<(), StorageError<u64>> {
        let bytes = snapshot.into_inner();
        let state: HeapState =
            serde_json::from_slice(&bytes).map_err(|e| StorageIOError::read(&e))?;

        {
            let mut sm = self.sm.write().await;
            sm.last_applied_log = meta.last_log_id;
            sm.last_membership = meta.last_membership.clone();
            sm.state = state;
        }

        {
            let mut current_snapshot = self.snapshot.write().await;
            *current_snapshot = Some(StoredSnapshot {
                meta: meta.clone(),
                data: bytes,
            });
        }

        Ok(())
    }

    async fn get_current_snapshot(&mut self) -> Result<Option<Snapshot<TypeConfig>>, StorageError<u64>> {
        let snapshot = self.snapshot.read().await;
        match &*snapshot {
            Some(s) => Ok(Some(Snapshot {
                meta: s.meta.clone(),
                snapshot: Box::new(Cursor::new(s.data.clone())),
            })),
            None => Ok(None),
        }
    }
}
