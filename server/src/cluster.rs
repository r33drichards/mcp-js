//! Raft-inspired cluster consensus for mcp-js.
//!
//! Provides leader election, log replication, and a replicated key-value store
//! backed by sled. Each node runs an HTTP server for Raft RPCs and a simple
//! data API.

use http_body_util::BodyExt;
use hyper::{body::Incoming, Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Notify, RwLock};
use tokio::time::{sleep, Instant};
use tokio_util::sync::CancellationToken;

const APPLIED_KEY: &[u8] = b"\0raft_applied";

// ============================================================================
// Types
// ============================================================================

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Role {
    Leader,
    Follower,
    Candidate,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogEntry {
    pub term: u64,
    pub index: u64,
    pub key: String,
    pub value: String,
    /// Additional key/value writes applied atomically with `key`/`value` when
    /// this entry commits. Lets a single replicated entry move a label head AND
    /// append its reflog entry as one all-or-nothing unit (see the
    /// `specs/FsLabelAtomicWrite` TLA+ model). Empty for ordinary single-key
    /// writes, and absent on the wire for backwards compatibility.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra: Vec<(String, String)>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppendEntriesRequest {
    pub term: u64,
    pub leader_id: String,
    pub prev_log_index: u64,
    pub prev_log_term: u64,
    pub entries: Vec<LogEntry>,
    pub leader_commit: u64,
    /// Current peer set from the leader, so followers can track membership
    /// changes. Absent (None) for backwards compatibility with older nodes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_addrs: Option<HashMap<String, String>>,
    /// Addresses of non-voting learner members, so followers compute the same
    /// voting set the leader does. Absent (None) for older nodes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub learner_addrs: Option<Vec<String>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AppendEntriesResponse {
    pub term: u64,
    pub success: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RequestVoteRequest {
    pub term: u64,
    pub candidate_id: String,
    pub last_log_index: u64,
    pub last_log_term: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RequestVoteResponse {
    pub term: u64,
    pub vote_granted: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClusterStatus {
    pub node_id: String,
    pub role: Role,
    pub term: u64,
    pub leader_id: Option<String>,
    pub commit_index: u64,
    pub last_applied: u64,
    pub log_length: u64,
    pub peers: Vec<String>,
    pub peer_addrs: HashMap<String, String>,
    /// Addresses of peers that are non-voting learners.
    #[serde(default)]
    pub learners: Vec<String>,
    /// Whether this node itself is a non-voting learner.
    #[serde(default)]
    pub is_learner: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PutRequest {
    pub key: String,
    pub value: String,
    /// Companion writes committed atomically with `key`/`value` (see
    /// `LogEntry::extra`). Absent on the wire for backwards compatibility.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra: Vec<(String, String)>,
}

/// Compare-and-set, forwarded to the leader. Advances `key` to
/// `new` only if its current committed/pending value equals `expected`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CasRequest {
    pub key: String,
    pub expected: Option<String>,
    pub new: String,
    /// Companion writes committed atomically with `new` when the compare matches
    /// (see `LogEntry::extra`). Absent on the wire for backwards compatibility.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra: Vec<(String, String)>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CasResponse {
    /// Whether the compare matched and the new value was committed.
    pub applied: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GetResponse {
    pub key: String,
    pub value: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JoinRequest {
    pub node_id: String,
    pub addr: String,
    /// Join as a non-voting learner instead of a full voting member.
    #[serde(default)]
    pub as_learner: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LeaveRequest {
    pub node_id: String,
}

// ============================================================================
// Configuration
// ============================================================================

#[derive(Clone, Debug)]
pub struct ClusterConfig {
    pub node_id: String,
    pub peers: Vec<String>, // "host:port" of other nodes
    /// Mapping from node_id → peer address (host:port).
    /// Populated when peers are specified as "id@host:port".
    pub peer_addrs: HashMap<String, String>,
    pub cluster_port: u16,
    /// The externally-reachable address of this node (e.g. "myhost:4000").
    /// Used so this node can advertise itself in the peer_addrs map.
    /// Defaults to "127.0.0.1:{cluster_port}" if not set.
    pub advertise_addr: Option<String>,
    pub heartbeat_interval: Duration,
    pub election_timeout_min: Duration,
    pub election_timeout_max: Duration,
    /// When true this node runs as a non-voting learner: it replicates the log
    /// but never starts elections, never grants votes, and is excluded from
    /// election and commit quorums. Intended for ephemeral nodes whose churn
    /// must not affect cluster availability.
    pub learner: bool,
}

impl ClusterConfig {
    /// Parse a peer list that may contain entries in either "host:port" or
    /// "node_id@host:port" format.  Returns (peers, peer_addrs) where peers
    /// is the list of addresses and peer_addrs maps node_id → address for
    /// entries that included an id.
    pub fn parse_peers(raw: &[String]) -> (Vec<String>, HashMap<String, String>) {
        let mut peers = Vec::new();
        let mut peer_addrs = HashMap::new();
        for entry in raw {
            if let Some((id, addr)) = entry.split_once('@') {
                peers.push(addr.to_string());
                peer_addrs.insert(id.to_string(), addr.to_string());
            } else {
                peers.push(entry.clone());
            }
        }
        (peers, peer_addrs)
    }
}

impl Default for ClusterConfig {
    fn default() -> Self {
        Self {
            node_id: "node1".to_string(),
            peers: Vec::new(),
            peer_addrs: HashMap::new(),
            cluster_port: 4000,
            advertise_addr: None,
            heartbeat_interval: Duration::from_millis(100),
            election_timeout_min: Duration::from_millis(300),
            election_timeout_max: Duration::from_millis(500),
            learner: false,
        }
    }
}

// ============================================================================
// Raft State
// ============================================================================

pub struct RaftState {
    pub role: Role,
    pub current_term: u64,
    pub voted_for: Option<String>,
    pub leader_id: Option<String>,
    pub log: Vec<LogEntry>,
    pub commit_index: u64,
    pub last_applied: u64,
    // Leader-only state
    pub next_index: HashMap<String, u64>,
    pub match_index: HashMap<String, u64>,
    // Timing
    pub last_heartbeat: Instant,
    // Dynamic peer set (addresses used for Raft RPCs)
    pub peers: Vec<String>,
    // node_id → address mapping for write forwarding and peer resolution.
    // Includes this node itself so followers can resolve any node id.
    pub peer_addrs: HashMap<String, String>,
    // Addresses (subset of `peers`) that are non-voting learners. They receive
    // replicated entries but are excluded from election and commit quorums.
    pub learners: std::collections::HashSet<String>,
}

impl RaftState {
    pub fn new() -> Self {
        Self {
            role: Role::Follower,
            current_term: 0,
            voted_for: None,
            leader_id: None,
            log: Vec::new(),
            commit_index: 0,
            last_applied: 0,
            next_index: HashMap::new(),
            match_index: HashMap::new(),
            last_heartbeat: Instant::now(),
            peers: Vec::new(),
            peer_addrs: HashMap::new(),
            learners: std::collections::HashSet::new(),
        }
    }

    pub fn last_log_index(&self) -> u64 {
        self.log.last().map(|e| e.index).unwrap_or(0)
    }

    pub fn last_log_term(&self) -> u64 {
        self.log.last().map(|e| e.term).unwrap_or(0)
    }

    /// Peer addresses that are voting members (excludes learners).
    pub fn voter_peers(&self) -> Vec<String> {
        self.peers
            .iter()
            .filter(|p| !self.learners.contains(*p))
            .cloned()
            .collect()
    }
}

// ============================================================================
// Cluster Node
// ============================================================================

pub struct ClusterNode {
    pub config: ClusterConfig,
    pub state: Arc<RwLock<RaftState>>,
    pub db: sled::Db,
    http_client: reqwest::Client,
    heartbeat_notify: Arc<Notify>,
    commit_notify: Arc<Notify>,
    shutdown: CancellationToken,
    fixed_voters: Option<HashMap<String, String>>,
    #[cfg(test)]
    fail_flush: std::sync::atomic::AtomicBool,
}

impl ClusterNode {
    pub fn new(config: ClusterConfig, db: sled::Db) -> Arc<Self> {
        let mut raft_state = RaftState::new();

        // A corrupt/unreadable consensus store must never bootstrap an empty voter.
        if let Some(data) = db.get("raft_meta").expect("read Raft metadata") {
            let meta: serde_json::Value =
                serde_json::from_slice(&data).expect("decode Raft metadata");
            raft_state.current_term = meta["current_term"].as_u64().expect("Raft term");
            raft_state.voted_for =
                serde_json::from_value(meta["voted_for"].clone()).expect("Raft vote");
        }
        let log_tree = db.open_tree("raft_log").expect("open Raft log");
        for item in log_tree.iter() {
            let (key, val) = item.expect("read Raft log");
            let entry: LogEntry = serde_json::from_slice(&val).expect("decode Raft log");
            assert_eq!(
                entry.index,
                raft_state.log.len() as u64 + 1,
                "noncontiguous Raft log"
            );
            assert_eq!(
                key.as_ref(),
                entry.index.to_be_bytes(),
                "invalid Raft log key"
            );
            assert!(
                entry.term <= raft_state.current_term,
                "log exceeds durable term"
            );
            raft_state.log.push(entry);
        }
        let data = db.open_tree("data").expect("open Raft data");
        if let Some(marker) = data.get(APPLIED_KEY).expect("read applied marker") {
            let (index, term): (u64, u64) =
                serde_json::from_slice(&marker).expect("decode applied marker");
            assert!(
                index > 0
                    && raft_state
                        .log
                        .get((index - 1) as usize)
                        .is_some_and(|e| e.term == term),
                "applied marker missing from log"
            );
            raft_state.commit_index = index;
            raft_state.last_applied = index;
        }
        let fixed_voters = db
            .get("raft_fixed_voters")
            .expect("read fixed voters")
            .map(|v| {
                serde_json::from_slice::<HashMap<String, String>>(&v).expect("decode fixed voters")
            });

        // Initialize peer set: start with CLI seed peers, then merge any
        // persisted peers (from previous dynamic joins).
        raft_state.peers = config.peers.clone();
        raft_state.peer_addrs = config.peer_addrs.clone();

        // Include self in peer_addrs so it gets propagated to followers
        let self_addr = config
            .advertise_addr
            .clone()
            .unwrap_or_else(|| format!("127.0.0.1:{}", config.cluster_port));
        raft_state
            .peer_addrs
            .insert(config.node_id.clone(), self_addr);

        if let Some(data) = db.get("raft_peers").expect("read persisted peers") {
            let persisted: HashMap<String, String> =
                serde_json::from_slice(&data).expect("decode persisted peers");
            for (id, addr) in persisted {
                raft_state.peer_addrs.entry(id).or_insert(addr);
            }
        }
        if let Some(data) = db.get("raft_learners").expect("read persisted learners") {
            raft_state.learners = serde_json::from_slice(&data).expect("decode persisted learners");
        }
        if let Some(voters) = &fixed_voters {
            let active: HashMap<_, _> = raft_state
                .peer_addrs
                .iter()
                .filter(|(id, addr)| {
                    !(raft_state.learners.contains(*addr)
                        || (config.learner && *id == &config.node_id))
                })
                .map(|(id, addr)| (id.clone(), addr.clone()))
                .collect();
            assert_eq!(
                &active, voters,
                "configured membership differs from sealed voters"
            );
            assert_eq!(
                voters.contains_key(&config.node_id),
                !config.learner,
                "cannot change voter role"
            );
        }
        if fixed_voters.is_some() {
            assert!(
                config
                    .peers
                    .iter()
                    .all(|addr| raft_state.peer_addrs.values().any(|known| known == addr)),
                "unknown address in sealed configuration"
            );
            raft_state.peers.clear();
        }
        // Count each peer once; never count self as a second vote.
        let own_addr = raft_state.peer_addrs.get(&config.node_id).cloned().unwrap();
        raft_state.peers.extend(
            raft_state
                .peer_addrs
                .iter()
                .filter(|(id, _)| *id != &config.node_id)
                .map(|(_, addr)| addr.clone()),
        );
        raft_state.peers.retain(|addr| addr != &own_addr);
        raft_state.peers.sort();
        raft_state.peers.dedup();

        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_millis(500))
            .connect_timeout(Duration::from_millis(200))
            .build()
            .expect("Failed to build HTTP client");

        Arc::new(Self {
            config,
            state: Arc::new(RwLock::new(raft_state)),
            db,
            http_client,
            heartbeat_notify: Arc::new(Notify::new()),
            commit_notify: Arc::new(Notify::new()),
            shutdown: CancellationToken::new(),
            fixed_voters,
            #[cfg(test)]
            fail_flush: std::sync::atomic::AtomicBool::new(false),
        })
    }

    /// Opt-in fixed voter set for scheduler fencing. All voters must use the
    /// same explicit id@address map. The durable seal prevents restart from
    /// silently reverting to dynamic membership. Learners may still join/leave.
    pub fn new_fixed(config: ClusterConfig, db: sled::Db) -> Result<Arc<Self>, String> {
        let mut voters = config.peer_addrs.clone();
        if config.peers.len() != voters.len()
            || config
                .peers
                .iter()
                .any(|addr| !voters.values().any(|v| v == addr))
        {
            return Err("fixed voters require unique id@address peers".into());
        }
        if !config.learner {
            if voters
                .insert(
                    config.node_id.clone(),
                    config
                        .advertise_addr
                        .clone()
                        .unwrap_or_else(|| format!("127.0.0.1:{}", config.cluster_port)),
                )
                .is_some()
            {
                return Err("self must not appear in peers".into());
            }
        }
        let unique: std::collections::HashSet<_> = voters.values().collect();
        if unique.len() != voters.len() || voters.is_empty() {
            return Err("voter addresses must be unique and nonempty".into());
        }
        if let Some(old) = db.get("raft_fixed_voters").map_err(|e| e.to_string())? {
            let old: HashMap<String, String> =
                serde_json::from_slice(&old).map_err(|e| e.to_string())?;
            if old != voters {
                return Err("cannot change sealed voters".into());
            }
        }
        // Never silently seal an existing dynamic cluster: membership and legacy
        // commit progress must be migrated with all voters offline by an operator.
        else if !db.is_empty()
            || !db
                .open_tree("raft_log")
                .map_err(|e| e.to_string())?
                .is_empty()
            || !db.open_tree("data").map_err(|e| e.to_string())?.is_empty()
        {
            return Err("fixed-voter mode requires a fresh consensus store".into());
        }
        db.insert("raft_fixed_voters", serde_json::to_vec(&voters).unwrap())
            .map_err(|e| e.to_string())?;
        db.flush().map_err(|e| e.to_string())?;
        let node = Self::new(config, db);
        Ok(node)
    }

    /// Start background tasks (election timer, heartbeat sender) and the
    /// cluster HTTP server.
    pub async fn start(self: &Arc<Self>) {
        let node = self.clone();
        tokio::spawn(async move { node.run_election_timer().await });

        let node = self.clone();
        tokio::spawn(async move { node.run_heartbeat().await });

        let node = self.clone();
        tokio::spawn(async move {
            if let Err(e) = start_cluster_server(node).await {
                tracing::error!("Cluster server error: {}", e);
            }
        });

        tracing::info!(
            "[{}] Cluster node started on port {}",
            self.config.node_id,
            self.config.cluster_port
        );
    }

    pub fn shutdown(&self) {
        self.shutdown.cancel();
    }

    // --- Persistence helpers ------------------------------------------------

    fn healthy(&self) -> Result<(), String> {
        if self.shutdown.is_cancelled() {
            Err("cluster stopped or storage failed; outcome may be unknown".into())
        } else {
            Ok(())
        }
    }

    fn durable<T>(&self, result: sled::Result<T>) -> Result<T, String> {
        #[cfg(test)]
        let result = if self.fail_flush.load(std::sync::atomic::Ordering::SeqCst) {
            Err(sled::Error::Unsupported("injected flush failure".into()))
        } else {
            result
        };
        match result.and_then(|value| self.db.flush().map(|_| value)) {
            Ok(value) => Ok(value),
            Err(error) => {
                self.shutdown.cancel();
                self.commit_notify.notify_waiters();
                tracing::error!("Raft storage failure; node stopped: {error}");
                Err(format!("Raft storage failed: {error}"))
            }
        }
    }

    fn persist_peers(&self, state: &RaftState) -> Result<(), String> {
        let mut batch = sled::Batch::default();
        batch.insert("raft_peers", serde_json::to_vec(&state.peer_addrs).unwrap());
        batch.insert(
            "raft_learners",
            serde_json::to_vec(&state.learners).unwrap(),
        );
        self.durable(self.db.apply_batch(batch))
    }

    fn persist_meta(&self, state: &RaftState) -> Result<(), String> {
        let meta =
            serde_json::json!({"current_term": state.current_term, "voted_for": state.voted_for});
        self.durable(
            self.db
                .insert("raft_meta", serde_json::to_vec(&meta).unwrap()),
        )
        .map(|_| ())
    }

    fn persist_log_entry(&self, entry: &LogEntry) -> Result<(), String> {
        self.durable(self.db.open_tree("raft_log").and_then(|tree| {
            tree.insert(
                entry.index.to_be_bytes(),
                serde_json::to_vec(entry).unwrap(),
            )
        }))
        .map(|_| ())
    }

    fn truncate_log_from(&self, from_index: u64) -> Result<(), String> {
        self.durable((|| {
            let tree = self.db.open_tree("raft_log")?;
            let mut batch = sled::Batch::default();
            for item in tree.range(from_index.to_be_bytes()..) {
                batch.remove(item?.0);
            }
            tree.apply_batch(batch)
        })())
    }

    fn valid_keys(key: &str, extra: &[(String, String)]) -> bool {
        !key.starts_with('\0') && extra.iter().all(|(key, _)| !key.starts_with('\0'))
    }

    async fn await_commit(&self, entry: &LogEntry) -> Result<(), String> {
        for _ in 0..100 {
            self.healthy()?;
            {
                let state = self.state.read().await;
                if state.role != Role::Leader || state.current_term != entry.term {
                    return Err("lost proposal leadership; outcome may be unknown".into());
                }
                if state.log.get((entry.index - 1) as usize) != Some(entry) {
                    return Err("proposal replaced".into());
                }
                if state.last_applied >= entry.index {
                    return Ok(());
                }
            }
            tokio::select! {
                _ = self.commit_notify.notified() => {},
                _ = sleep(Duration::from_millis(50)) => {},
                _ = self.shutdown.cancelled() => {},
            }
        }
        Err("timeout waiting for durable commit; outcome may be unknown".into())
    }

    // --- Election -----------------------------------------------------------

    async fn run_election_timer(self: Arc<Self>) {
        loop {
            let timeout = {
                let mut rng = rand::thread_rng();
                Duration::from_millis(rng.gen_range(
                    self.config.election_timeout_min.as_millis() as u64
                        ..=self.config.election_timeout_max.as_millis() as u64,
                ))
            };

            tokio::select! {
                _ = sleep(timeout) => {
                    // Learners never start elections; they only follow.
                    if self.config.learner {
                        continue;
                    }
                    let state = self.state.read().await;
                    if state.role == Role::Leader {
                        continue;
                    }
                    if state.last_heartbeat.elapsed() >= timeout {
                        drop(state);
                        self.start_election().await;
                    }
                }
                _ = self.heartbeat_notify.notified() => {
                    // Heartbeat received, reset timer
                    continue;
                }
                _ = self.shutdown.cancelled() => {
                    return;
                }
            }
        }
    }

    async fn start_election(self: &Arc<Self>) {
        let (term, last_log_index, last_log_term, peers, voter_count) = {
            let mut state = self.state.write().await;
            state.current_term += 1;
            state.role = Role::Candidate;
            state.voted_for = Some(self.config.node_id.clone());
            state.leader_id = None;
            if self.healthy().is_err() || self.persist_meta(&state).is_err() {
                return;
            }
            (
                state.current_term,
                state.last_log_index(),
                state.last_log_term(),
                state.voter_peers(),
                state.voter_peers().len(),
            )
        };

        tracing::info!(
            "[{}] Starting election for term {}",
            self.config.node_id,
            term
        );

        // Only voting members count toward the election quorum. Learners are
        // sent RequestVote (harmless) but refuse, so they cannot inflate votes.
        let majority = (voter_count + 1) / 2 + 1;
        let mut votes: usize = 1; // vote for self

        // Send RequestVote RPCs to all peers in parallel
        let mut handles = Vec::new();
        for peer in &peers {
            let req = RequestVoteRequest {
                term,
                candidate_id: self.config.node_id.clone(),
                last_log_index,
                last_log_term,
            };
            let client = self.http_client.clone();
            let url = format!("http://{}/raft/request-vote", peer);
            handles.push(tokio::spawn(async move {
                client
                    .post(&url)
                    .json(&req)
                    .send()
                    .await
                    .ok()
                    .and_then(|r| futures::executor::block_on(r.json::<RequestVoteResponse>()).ok())
            }));
        }

        for handle in handles {
            if let Ok(Some(resp)) = handle.await {
                let mut state = self.state.write().await;
                if resp.term > state.current_term {
                    state.current_term = resp.term;
                    state.role = Role::Follower;
                    state.voted_for = None;
                    state.leader_id = None;
                    let _ = self.persist_meta(&state);
                    return;
                }
                if self.healthy().is_err()
                    || state.current_term != term
                    || state.role != Role::Candidate
                {
                    return;
                }
                if resp.term == term && resp.vote_granted {
                    votes += 1;
                }
            }
        }

        if votes >= majority {
            let mut state = self.state.write().await;
            if state.current_term == term && state.role == Role::Candidate {
                state.role = Role::Leader;
                state.leader_id = Some(self.config.node_id.clone());
                let last_index = state.last_log_index();
                for peer in &state.peers.clone() {
                    state.next_index.insert(peer.clone(), last_index + 1);
                    state.match_index.insert(peer.clone(), 0);
                }
                if self.persist_meta(&state).is_err() {
                    return;
                }
                tracing::info!(
                    "[{}] Won election for term {} ({}/{} voter votes)",
                    self.config.node_id,
                    term,
                    votes,
                    voter_count + 1
                );
            }
        }
    }

    // --- Heartbeat / Replication --------------------------------------------

    async fn run_heartbeat(self: Arc<Self>) {
        loop {
            tokio::select! {
                _ = sleep(self.config.heartbeat_interval) => {
                    let is_leader = {
                        let state = self.state.read().await;
                        state.role == Role::Leader
                    };
                    if is_leader {
                        self.send_append_entries_to_all().await;
                    }
                }
                _ = self.shutdown.cancelled() => {
                    return;
                }
            }
        }
    }

    async fn send_append_entries_to_all(self: &Arc<Self>) {
        let peers = {
            let state = self.state.read().await;
            state.peers.clone()
        };
        let mut handles = Vec::new();

        for peer in peers {
            let node = self.clone();
            handles.push(tokio::spawn(async move {
                node.send_append_entries_to_peer(&peer).await
            }));
        }

        for handle in handles {
            let _ = handle.await;
        }

        // Update commit index based on match_index
        self.update_commit_index().await;
    }

    async fn send_append_entries_to_peer(self: &Arc<Self>, peer: &str) {
        let (
            term,
            leader_id,
            prev_log_index,
            prev_log_term,
            entries,
            leader_commit,
            current_peer_addrs,
            current_learners,
        ) = {
            let state = self.state.read().await;
            if state.role != Role::Leader {
                return;
            }
            let next_idx = state.next_index.get(peer).copied().unwrap_or(1);
            let prev_idx = if next_idx > 0 { next_idx - 1 } else { 0 };
            let prev_term = if prev_idx > 0 {
                state
                    .log
                    .get((prev_idx - 1) as usize)
                    .map(|e| e.term)
                    .unwrap_or(0)
            } else {
                0
            };
            let entries: Vec<LogEntry> = state
                .log
                .iter()
                .filter(|e| e.index >= next_idx)
                .cloned()
                .collect();
            // peer_addrs already includes self (set during initialization)
            (
                state.current_term,
                self.config.node_id.clone(),
                prev_idx,
                prev_term,
                entries,
                state.commit_index,
                state.peer_addrs.clone(),
                state.learners.iter().cloned().collect::<Vec<String>>(),
            )
        };

        let req = AppendEntriesRequest {
            term,
            leader_id,
            prev_log_index,
            prev_log_term,
            entries: entries.clone(),
            leader_commit,
            peer_addrs: Some(current_peer_addrs),
            learner_addrs: Some(current_learners),
        };

        let url = format!("http://{}/raft/append-entries", peer);
        let resp = self.http_client.post(&url).json(&req).send().await;

        match resp {
            Ok(r) => {
                if let Ok(ae_resp) = r.json::<AppendEntriesResponse>().await {
                    self.accept_append_response(
                        peer,
                        term,
                        prev_log_index,
                        entries.last().map_or(prev_log_index, |e| e.index),
                        ae_resp,
                    )
                    .await;
                }
            }
            Err(_) => {
                // Peer unreachable, will retry on next heartbeat
            }
        }
    }

    async fn accept_append_response(
        &self,
        peer: &str,
        term: u64,
        prev_log_index: u64,
        matched_index: u64,
        ae_resp: AppendEntriesResponse,
    ) {
        let mut state = self.state.write().await;
        if ae_resp.term > state.current_term {
            state.current_term = ae_resp.term;
            state.role = Role::Follower;
            state.voted_for = None;
            state.leader_id = None;
            let _ = self.persist_meta(&state);
            return;
        }
        if self.healthy().is_err()
            || state.role != Role::Leader
            || state.current_term != term
            || ae_resp.term != term
        {
            return;
        }
        let matched = matched_index;
        if ae_resp.success {
            let old = state.match_index.get(peer).copied().unwrap_or(0);
            state.match_index.insert(peer.to_string(), old.max(matched));
            let next = state.next_index.get(peer).copied().unwrap_or(1);
            state
                .next_index
                .insert(peer.to_string(), next.max(matched + 1));
        } else {
            let next = state.next_index.get(peer).copied().unwrap_or(1);
            if next == prev_log_index + 1 && next > 1 {
                state.next_index.insert(peer.to_string(), next - 1);
            }
        }
    }

    async fn update_commit_index(self: &Arc<Self>) {
        let mut state = self.state.write().await;
        if self.healthy().is_err() || state.role != Role::Leader {
            return;
        }

        // Only voting members count toward the commit quorum. Learners are
        // still replicated to (they remain in `peers`), but a write commits as
        // soon as a majority of voters — including the leader — has it.
        let voter_peers = state.voter_peers();
        let majority = (voter_peers.len() + 1) / 2 + 1;

        // Find the highest N such that a majority of match_index[i] >= N
        // and log[N].term == currentTerm
        let last_idx = state.last_log_index();
        for n in (state.commit_index + 1..=last_idx).rev() {
            let mut replication_count: usize = 1; // count self
            for peer in &voter_peers {
                if state.match_index.get(peer).copied().unwrap_or(0) >= n {
                    replication_count += 1;
                }
            }
            if replication_count >= majority {
                if let Some(entry) = state.log.get((n - 1) as usize) {
                    if entry.term == state.current_term {
                        state.commit_index = n;
                        self.commit_notify.notify_waiters();
                        break;
                    }
                }
            }
        }

        if self.apply_committed(&mut state).is_ok() {
            self.commit_notify.notify_waiters();
        }
    }

    fn apply_committed(&self, state: &mut RaftState) -> Result<(), String> {
        while state.last_applied < state.commit_index {
            let entry = &state.log[state.last_applied as usize];
            self.apply_log_entry(entry)?;
            state.last_applied += 1;
        }
        Ok(())
    }

    // The progress marker and all companion writes are one atomic batch.
    // A successful ACK requires a durable flush of both data and marker.
    fn apply_log_entry(&self, entry: &LogEntry) -> Result<(), String> {
        self.durable((|| {
            let tree = self.db.open_tree("data")?;
            let mut batch = sled::Batch::default();
            batch.insert(entry.key.as_bytes(), entry.value.as_bytes());
            for (key, value) in &entry.extra {
                batch.insert(key.as_bytes(), value.as_bytes());
            }
            batch.insert(
                APPLIED_KEY,
                serde_json::to_vec(&(entry.index, entry.term)).unwrap(),
            );
            tree.apply_batch(batch)
        })())
    }

    // --- RPC Handlers -------------------------------------------------------

    pub async fn handle_append_entries(&self, req: AppendEntriesRequest) -> AppendEntriesResponse {
        let mut state = self.state.write().await;
        let reject = |term| AppendEntriesResponse {
            term,
            success: false,
        };
        if self.healthy().is_err() || req.term < state.current_term {
            return reject(state.current_term);
        }
        if let Some(voters) = &self.fixed_voters {
            if !voters.contains_key(&req.leader_id) {
                return reject(state.current_term);
            }
            if let Some(peers) = &req.peer_addrs {
                let learners = req.learner_addrs.clone().unwrap_or_default();
                let incoming: HashMap<_, _> = peers
                    .iter()
                    .filter(|(_, addr)| !learners.contains(addr))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                if &incoming != voters {
                    return reject(state.current_term);
                }
            }
        }
        if req.term > state.current_term {
            state.current_term = req.term;
            state.voted_for = None;
        }
        state.role = Role::Follower;
        state.leader_id = Some(req.leader_id.clone());
        if self.persist_meta(&state).is_err() {
            return reject(state.current_term);
        }
        state.last_heartbeat = Instant::now();
        self.heartbeat_notify.notify_one();
        if (req.prev_log_index == 0 && req.prev_log_term != 0)
            || (req.prev_log_index > 0
                && !state
                    .log
                    .get((req.prev_log_index - 1) as usize)
                    .is_some_and(|e| e.term == req.prev_log_term))
        {
            return reject(state.current_term);
        }
        for (offset, entry) in req.entries.iter().enumerate() {
            if req.prev_log_index.checked_add(offset as u64 + 1) != Some(entry.index)
                || entry.term > req.term
                || entry.term == 0
                || !Self::valid_keys(&entry.key, &entry.extra)
            {
                return reject(state.current_term);
            }
            if let Some(old) = state.log.get((entry.index - 1) as usize) {
                if old != entry && (entry.index <= state.commit_index || old.term == entry.term) {
                    return reject(state.current_term);
                }
            }
        }
        for entry in &req.entries {
            let index = (entry.index - 1) as usize;
            if state
                .log
                .get(index)
                .is_some_and(|old| old.term != entry.term)
            {
                if self.truncate_log_from(entry.index).is_err() {
                    return reject(state.current_term);
                }
                state.log.truncate(index);
            }
            if index == state.log.len() {
                if self.persist_log_entry(entry).is_err() {
                    return reject(state.current_term);
                }
                state.log.push(entry.clone());
            }
        }
        // Do not commit an unvalidated local suffix on a short heartbeat.
        let matched = req.prev_log_index + req.entries.len() as u64;
        state.commit_index = state.commit_index.max(req.leader_commit.min(matched));
        if self.apply_committed(&mut state).is_err() {
            return reject(state.current_term);
        }
        // Legacy dynamic membership remains available outside fixed-voter mode.
        if let Some(peers) = req.peer_addrs {
            state.peer_addrs = peers;
            state.peers = state
                .peer_addrs
                .iter()
                .filter(|(id, _)| *id != &self.config.node_id)
                .map(|(_, addr)| addr.clone())
                .collect();
            state.peers.sort();
            state.peers.dedup();
            if let Some(learners) = req.learner_addrs {
                state.learners = learners.into_iter().collect();
            }
            if self.persist_peers(&state).is_err() {
                return reject(state.current_term);
            }
        }
        AppendEntriesResponse {
            term: state.current_term,
            success: true,
        }
    }

    pub async fn handle_request_vote(&self, req: RequestVoteRequest) -> RequestVoteResponse {
        let mut state = self.state.write().await;

        if self.healthy().is_err()
            || self
                .fixed_voters
                .as_ref()
                .is_some_and(|voters| !voters.contains_key(&req.candidate_id))
        {
            return RequestVoteResponse {
                term: state.current_term,
                vote_granted: false,
            };
        }
        // Learners are non-voting: they never grant votes, so a candidate that
        // (incorrectly) counted a learner's vote could not reach quorum from it.
        if self.config.learner {
            // Still adopt a higher term so we stay a passive follower.
            if req.term > state.current_term {
                state.current_term = req.term;
                state.voted_for = None;
                let _ = self.persist_meta(&state);
            }
            return RequestVoteResponse {
                term: state.current_term,
                vote_granted: false,
            };
        }

        if req.term < state.current_term {
            return RequestVoteResponse {
                term: state.current_term,
                vote_granted: false,
            };
        }

        if req.term > state.current_term {
            state.current_term = req.term;
            state.role = Role::Follower;
            state.voted_for = None;
            state.leader_id = None;
        }

        let can_vote =
            state.voted_for.is_none() || state.voted_for.as_ref() == Some(&req.candidate_id);

        let log_ok = req.last_log_term > state.last_log_term()
            || (req.last_log_term == state.last_log_term()
                && req.last_log_index >= state.last_log_index());

        if can_vote && log_ok {
            state.voted_for = Some(req.candidate_id.clone());
            state.last_heartbeat = Instant::now();
            self.heartbeat_notify.notify_one();
            let durable = self.persist_meta(&state).is_ok();
            RequestVoteResponse {
                term: state.current_term,
                vote_granted: durable,
            }
        } else {
            let _ = self.persist_meta(&state);
            RequestVoteResponse {
                term: state.current_term,
                vote_granted: false,
            }
        }
    }

    // --- Client Data Operations ---------------------------------------------

    /// Write a key-value pair. Must be called on the leader.
    pub async fn put(&self, key: String, value: String) -> Result<(), String> {
        self.put_with(key, value, Vec::new()).await
    }

    /// Leader-only blind write that also applies `extra` key/value pairs in the
    /// same committed entry, so all writes land atomically (all-or-nothing).
    pub async fn put_with(
        &self,
        key: String,
        value: String,
        extra: Vec<(String, String)>,
    ) -> Result<(), String> {
        self.healthy()?;
        if !Self::valid_keys(&key, &extra) {
            return Err("reserved consensus key".into());
        }
        let entry = {
            let mut state = self.state.write().await;
            if state.role != Role::Leader {
                return Err(format!(
                    "not the leader; current leader: {:?}",
                    state.leader_id
                ));
            }
            let entry = LogEntry {
                term: state.current_term,
                index: state.last_log_index() + 1,
                key,
                value,
                extra,
            };
            self.persist_meta(&state)?;
            self.persist_log_entry(&entry)?;
            state.log.push(entry.clone());
            entry
        };

        self.await_commit(&entry).await
    }

    /// Write a key-value pair, forwarding to the leader if this node is a
    /// follower.  Uses the dynamic peer_addrs table to resolve the leader's
    /// network address from its node-id.
    pub async fn put_or_forward(&self, key: String, value: String) -> Result<(), String> {
        self.put_with_or_forward(key, value, Vec::new()).await
    }

    /// `put_with`, forwarding to the leader when this node is a follower. The
    /// `extra` writes are carried through so they commit atomically with the
    /// primary write on the leader.
    pub async fn put_with_or_forward(
        &self,
        key: String,
        value: String,
        extra: Vec<(String, String)>,
    ) -> Result<(), String> {
        self.healthy()?;
        if !Self::valid_keys(&key, &extra) {
            return Err("reserved consensus key".into());
        }
        let (is_leader, leader_id, leader_addr) = {
            let state = self.state.read().await;
            let lid = state.leader_id.clone();
            let addr = lid
                .as_ref()
                .and_then(|id| state.peer_addrs.get(id).cloned());
            (state.role == Role::Leader, lid, addr)
        };

        if is_leader {
            return self.put_with(key, value, extra).await;
        }

        // We are not the leader – try to forward.
        match leader_id {
            Some(id) => {
                let leader_addr = leader_addr
                    .ok_or_else(|| format!("unknown leader address for node '{}'", id))?;
                let url = format!("http://{}/data/put", leader_addr);
                let req = PutRequest { key, value, extra };
                let resp = self
                    .http_client
                    .post(&url)
                    .json(&req)
                    .send()
                    .await
                    .map_err(|e| format!("failed to forward to leader: {}", e))?;
                if resp.status().is_success() {
                    Ok(())
                } else {
                    let body = resp.text().await.unwrap_or_default();
                    Err(format!("leader returned error: {}", body))
                }
            }
            None => Err("no leader elected yet".to_string()),
        }
    }

    /// Leader-only compare-and-set. The compare is made under the
    /// state lock against the latest *pending or committed* value of `key` (so
    /// two concurrent CAS serialize correctly: the first to append shifts the
    /// value the second sees). Returns `Ok(true)` when the new value was
    /// appended and durably applied. `Ok(false)` is a provisional mismatch
    /// against pending state, not a linearizable read or proof of ownership.
    pub async fn cas(
        &self,
        key: String,
        expected: Option<String>,
        new: String,
    ) -> Result<bool, String> {
        self.cas_with(key, expected, new, Vec::new()).await
    }

    /// Compare-and-set that, when the compare matches, also applies
    /// `extra` key/value writes in the same committed entry. The compare is made
    /// against `key` only; `extra` writes are unconditional companions (e.g. a
    /// reflog append that must land atomically with the head move it records).
    pub async fn cas_with(
        &self,
        key: String,
        expected: Option<String>,
        new: String,
        extra: Vec<(String, String)>,
    ) -> Result<bool, String> {
        self.healthy()?;
        if !Self::valid_keys(&key, &extra) {
            return Err("reserved consensus key".into());
        }
        let entry = {
            let mut state = self.state.write().await;
            if state.role != Role::Leader {
                return Err(format!(
                    "not the leader; current leader: {:?}",
                    state.leader_id
                ));
            }
            // Latest value = most recent log entry for the key (covers
            // appended-but-not-yet-committed writes), else the applied value.
            let current = state.log.iter().rev().find_map(|e| {
                e.extra
                    .iter()
                    .rev()
                    .find(|(k, _)| k == &key)
                    .map(|(_, v)| v.clone())
                    .or_else(|| (e.key == key).then(|| e.value.clone()))
            });
            let current = match current {
                Some(value) => Some(value),
                None => self.read_applied(&key)?,
            };
            if current.as_deref() != expected.as_deref() {
                return Ok(false);
            }
            let entry = LogEntry {
                term: state.current_term,
                index: state.last_log_index() + 1,
                key,
                value: new,
                extra,
            };
            self.persist_meta(&state)?;
            self.persist_log_entry(&entry)?;
            state.log.push(entry.clone());
            entry
        };

        self.await_commit(&entry).await.map(|_| true)
    }

    /// Compare-and-set, forwarding to the leader when this node is a follower.
    pub async fn cas_or_forward(
        &self,
        key: String,
        expected: Option<String>,
        new: String,
    ) -> Result<bool, String> {
        self.cas_with_or_forward(key, expected, new, Vec::new())
            .await
    }

    /// `cas_with`, forwarding to the leader when this node is a follower. The
    /// `extra` writes are carried through so, on a matching compare, they commit
    /// atomically with the new value on the leader.
    pub async fn cas_with_or_forward(
        &self,
        key: String,
        expected: Option<String>,
        new: String,
        extra: Vec<(String, String)>,
    ) -> Result<bool, String> {
        self.healthy()?;
        if !Self::valid_keys(&key, &extra) {
            return Err("reserved consensus key".into());
        }
        let (is_leader, leader_id, leader_addr) = {
            let state = self.state.read().await;
            let lid = state.leader_id.clone();
            let addr = lid
                .as_ref()
                .and_then(|id| state.peer_addrs.get(id).cloned());
            (state.role == Role::Leader, lid, addr)
        };

        if is_leader {
            return self.cas_with(key, expected, new, extra).await;
        }

        match leader_id {
            Some(id) => {
                let leader_addr = leader_addr
                    .ok_or_else(|| format!("unknown leader address for node '{}'", id))?;
                let url = format!("http://{}/data/cas", leader_addr);
                let req = CasRequest {
                    key,
                    expected,
                    new,
                    extra,
                };
                let resp = self
                    .http_client
                    .post(&url)
                    .json(&req)
                    .send()
                    .await
                    .map_err(|e| format!("failed to forward cas to leader: {}", e))?;
                if resp.status().is_success() {
                    let body: CasResponse = resp
                        .json()
                        .await
                        .map_err(|e| format!("invalid cas response: {}", e))?;
                    Ok(body.applied)
                } else {
                    let body = resp.text().await.unwrap_or_default();
                    Err(format!("leader returned error: {}", body))
                }
            }
            None => Err("no leader elected yet".to_string()),
        }
    }

    /// Read the applied (committed) value of a key from the state-machine tree.
    fn read_applied(&self, key: &str) -> Result<Option<String>, String> {
        let result = self
            .db
            .open_tree("data")
            .and_then(|tree| tree.get(key.as_bytes()));
        match result {
            Ok(value) => value
                .map(|v| String::from_utf8(v.to_vec()).map_err(|e| e.to_string()))
                .transpose(),
            Err(error) => {
                self.shutdown.cancel();
                Err(error.to_string())
            }
        }
    }

    // --- Dynamic Membership ------------------------------------------------

    /// Add a peer to the cluster.  Must be called on the leader.
    /// The peer is immediately added to the active peer set and persisted.
    /// When `as_learner` is true the peer is replicated to but excluded from
    /// election and commit quorums.
    pub async fn add_peer(
        &self,
        node_id: String,
        addr: String,
        as_learner: bool,
    ) -> Result<(), String> {
        self.healthy()?;
        let mut state = self.state.write().await;
        if state.role != Role::Leader {
            return Err(format!(
                "not the leader; current leader: {:?}",
                state.leader_id
            ));
        }

        if node_id == self.config.node_id
            || state
                .peer_addrs
                .iter()
                .any(|(id, old)| id != &node_id && old == &addr)
        {
            return Err("duplicate peer identity/address".into());
        }
        if self.fixed_voters.is_some() && !as_learner {
            return Err("fixed voter membership; only learner joins allowed".into());
        }
        // Idempotent: skip if already present
        if state.peer_addrs.contains_key(&node_id) {
            return Ok(());
        }

        state.peers.push(addr.clone());
        state.peer_addrs.insert(node_id.clone(), addr.clone());
        if as_learner {
            state.learners.insert(addr.clone());
        }

        // Initialize replication state for the new peer
        let last_index = state.last_log_index();
        state.next_index.insert(addr.clone(), last_index + 1);
        state.match_index.insert(addr.clone(), 0);

        self.persist_peers(&state)?;
        tracing::info!(
            "[{}] Added {} {}@{}",
            self.config.node_id,
            if as_learner { "learner" } else { "peer" },
            node_id,
            addr
        );
        Ok(())
    }

    /// Remove a peer from the cluster.  Must be called on the leader.
    pub async fn remove_peer(&self, node_id: String) -> Result<(), String> {
        self.healthy()?;
        let mut state = self.state.write().await;
        if state.role != Role::Leader {
            return Err(format!(
                "not the leader; current leader: {:?}",
                state.leader_id
            ));
        }

        if self
            .fixed_voters
            .as_ref()
            .is_some_and(|voters| voters.contains_key(&node_id))
        {
            return Err("cannot remove a fixed voter".into());
        }
        if let Some(addr) = state.peer_addrs.remove(&node_id) {
            state.peers.retain(|p| p != &addr);
            state.learners.remove(&addr);
            state.next_index.remove(&addr);
            state.match_index.remove(&addr);
            self.persist_peers(&state)?;
            tracing::info!(
                "[{}] Removed peer {}@{}",
                self.config.node_id,
                node_id,
                addr
            );
            Ok(())
        } else {
            Err(format!("peer '{}' not found", node_id))
        }
    }

    /// Handle a join request.  If this node is the leader it adds the peer
    /// directly; otherwise it forwards to the leader.
    pub async fn handle_join(&self, req: JoinRequest) -> Result<(), String> {
        self.healthy()?;
        {
            let state = self.state.read().await;
            if state.role == Role::Leader {
                drop(state);
                return self.add_peer(req.node_id, req.addr, req.as_learner).await;
            }
        }

        // Forward to the leader
        let (leader_id, leader_addr) = {
            let state = self.state.read().await;
            let lid = state.leader_id.clone();
            let addr = lid
                .as_ref()
                .and_then(|id| state.peer_addrs.get(id).cloned());
            (lid, addr)
        };

        match (leader_id, leader_addr) {
            (Some(_), Some(addr)) => {
                let url = format!("http://{}/raft/join", addr);
                let resp = self
                    .http_client
                    .post(&url)
                    .json(&req)
                    .send()
                    .await
                    .map_err(|e| format!("failed to forward join to leader: {}", e))?;
                if resp.status().is_success() {
                    Ok(())
                } else {
                    let body = resp.text().await.unwrap_or_default();
                    Err(format!("leader returned error: {}", body))
                }
            }
            _ => Err("no leader elected yet".to_string()),
        }
    }

    /// Handle a leave request.  If this node is the leader it removes the
    /// peer directly; otherwise it forwards to the leader.
    pub async fn handle_leave(&self, req: LeaveRequest) -> Result<(), String> {
        self.healthy()?;
        {
            let state = self.state.read().await;
            if state.role == Role::Leader {
                drop(state);
                return self.remove_peer(req.node_id).await;
            }
        }

        // Forward to the leader
        let (leader_id, leader_addr) = {
            let state = self.state.read().await;
            let lid = state.leader_id.clone();
            let addr = lid
                .as_ref()
                .and_then(|id| state.peer_addrs.get(id).cloned());
            (lid, addr)
        };

        match (leader_id, leader_addr) {
            (Some(_), Some(addr)) => {
                let url = format!("http://{}/raft/leave", addr);
                let resp = self
                    .http_client
                    .post(&url)
                    .json(&req)
                    .send()
                    .await
                    .map_err(|e| format!("failed to forward leave to leader: {}", e))?;
                if resp.status().is_success() {
                    Ok(())
                } else {
                    let body = resp.text().await.unwrap_or_default();
                    Err(format!("leader returned error: {}", body))
                }
            }
            _ => Err("no leader elected yet".to_string()),
        }
    }

    /// Read a key. Can be called on any node (returns locally committed data).
    pub async fn get(&self, key: &str) -> Result<Option<String>, String> {
        let data_tree = self.db.open_tree("data").map_err(|e| e.to_string())?;
        match data_tree.get(key.as_bytes()) {
            Ok(Some(value)) => Ok(Some(String::from_utf8_lossy(&value).to_string())),
            Ok(None) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    /// Scan all keys with the given prefix from the replicated data tree.
    /// Returns (key, value) pairs sorted lexicographically.
    pub fn scan_prefix(&self, prefix: &str) -> Result<Vec<(String, String)>, String> {
        let data_tree = self.db.open_tree("data").map_err(|e| e.to_string())?;
        let mut results = Vec::new();
        for item in data_tree.scan_prefix(prefix.as_bytes()) {
            let (key, value) = item.map_err(|e| e.to_string())?;
            let key_str = String::from_utf8_lossy(&key).to_string();
            let val_str = String::from_utf8_lossy(&value).to_string();
            results.push((key_str, val_str));
        }
        Ok(results)
    }

    /// Return the current status of this node.
    pub async fn status(&self) -> ClusterStatus {
        let state = self.state.read().await;
        ClusterStatus {
            node_id: self.config.node_id.clone(),
            role: state.role,
            term: state.current_term,
            leader_id: state.leader_id.clone(),
            commit_index: state.commit_index,
            last_applied: state.last_applied,
            log_length: state.log.len() as u64,
            peers: state.peers.clone(),
            peer_addrs: state.peer_addrs.clone(),
            learners: state.learners.iter().cloned().collect(),
            is_learner: self.config.learner,
        }
    }
}

// ============================================================================
// HTTP Server for Raft RPCs + Data API
// ============================================================================

async fn read_body(req: Request<Incoming>) -> Result<Vec<u8>, String> {
    req.into_body()
        .collect()
        .await
        .map(|c| c.to_bytes().to_vec())
        .map_err(|e| e.to_string())
}

fn json_response<T: Serialize>(status: u16, body: &T) -> Response<String> {
    let json = serde_json::to_string(body).unwrap_or_else(|_| "{}".to_string());
    Response::builder()
        .status(StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR))
        .header("content-type", "application/json")
        .body(json)
        .unwrap()
}

fn error_response(status: u16, msg: &str) -> Response<String> {
    Response::builder()
        .status(StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR))
        .header("content-type", "application/json")
        .body(serde_json::json!({"error": msg}).to_string())
        .unwrap()
}

async fn route(
    node: Arc<ClusterNode>,
    req: Request<Incoming>,
) -> Result<Response<String>, Infallible> {
    // Reject all requests on a shut-down node.  The TCP listener stops
    // accepting new connections on shutdown, but already-established HTTP/1.1
    // keep-alive connections (held by reqwest's connection pool) can still
    // deliver requests.  Without this guard a "dead" node can grant votes or
    // acknowledge replication, breaking quorum guarantees.
    if node.shutdown.is_cancelled() {
        return Ok(error_response(503, "node is shut down"));
    }

    let method = req.method().clone();
    let path = req.uri().path().to_string();

    let response =
        match (method, path.as_str()) {
            (Method::POST, "/raft/append-entries") => {
                match read_body(req).await.and_then(|b| {
                    serde_json::from_slice::<AppendEntriesRequest>(&b).map_err(|e| e.to_string())
                }) {
                    Ok(ae_req) => {
                        let resp = node.handle_append_entries(ae_req).await;
                        json_response(200, &resp)
                    }
                    Err(e) => error_response(400, &e),
                }
            }

            (Method::POST, "/raft/request-vote") => {
                match read_body(req).await.and_then(|b| {
                    serde_json::from_slice::<RequestVoteRequest>(&b).map_err(|e| e.to_string())
                }) {
                    Ok(rv_req) => {
                        let resp = node.handle_request_vote(rv_req).await;
                        json_response(200, &resp)
                    }
                    Err(e) => error_response(400, &e),
                }
            }

            (Method::GET, "/raft/status") => {
                let status = node.status().await;
                json_response(200, &status)
            }

            (Method::POST, "/raft/join") => {
                match read_body(req).await.and_then(|b| {
                    serde_json::from_slice::<JoinRequest>(&b).map_err(|e| e.to_string())
                }) {
                    Ok(join_req) => match node.handle_join(join_req).await {
                        Ok(()) => json_response(200, &serde_json::json!({"ok": true})),
                        Err(e) => error_response(503, &e),
                    },
                    Err(e) => error_response(400, &e),
                }
            }

            (Method::POST, "/raft/leave") => {
                match read_body(req).await.and_then(|b| {
                    serde_json::from_slice::<LeaveRequest>(&b).map_err(|e| e.to_string())
                }) {
                    Ok(leave_req) => match node.handle_leave(leave_req).await {
                        Ok(()) => json_response(200, &serde_json::json!({"ok": true})),
                        Err(e) => error_response(503, &e),
                    },
                    Err(e) => error_response(400, &e),
                }
            }

            (Method::POST, "/data/put") => {
                match read_body(req).await.and_then(|b| {
                    serde_json::from_slice::<PutRequest>(&b).map_err(|e| e.to_string())
                }) {
                    Ok(put_req) => match node
                        .put_with_or_forward(put_req.key, put_req.value, put_req.extra)
                        .await
                    {
                        Ok(()) => json_response(200, &serde_json::json!({"ok": true})),
                        Err(e) => error_response(503, &e),
                    },
                    Err(e) => error_response(400, &e),
                }
            }

            (Method::POST, "/data/cas") => {
                match read_body(req).await.and_then(|b| {
                    serde_json::from_slice::<CasRequest>(&b).map_err(|e| e.to_string())
                }) {
                    Ok(cas_req) => {
                        match node
                            .cas_with_or_forward(
                                cas_req.key,
                                cas_req.expected,
                                cas_req.new,
                                cas_req.extra,
                            )
                            .await
                        {
                            Ok(applied) => json_response(200, &CasResponse { applied }),
                            Err(e) => error_response(503, &e),
                        }
                    }
                    Err(e) => error_response(400, &e),
                }
            }

            (ref m, p) if *m == Method::GET && p.starts_with("/data/get/") => {
                let key = &p["/data/get/".len()..];
                match node.get(key).await {
                    Ok(value) => json_response(
                        200,
                        &GetResponse {
                            key: key.to_string(),
                            value,
                        },
                    ),
                    Err(e) => error_response(500, &e),
                }
            }

            _ => error_response(404, "not found"),
        };

    Ok(response)
}

pub async fn start_cluster_server(
    node: Arc<ClusterNode>,
) -> Result<(), Box<dyn std::error::Error>> {
    let addr = format!("0.0.0.0:{}", node.config.cluster_port);
    let tcp_listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!(
        "[{}] Cluster HTTP server listening on {}",
        node.config.node_id,
        addr
    );

    let shutdown = node.shutdown.clone();

    loop {
        tokio::select! {
            accept = tcp_listener.accept() => {
                let (stream, _addr) = accept?;
                let node = node.clone();

                let service = hyper::service::service_fn(move |req| {
                    let node = node.clone();
                    async move { route(node, req).await }
                });

                tokio::spawn(async move {
                    let conn = hyper::server::conn::http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service);
                    if let Err(err) = conn.await {
                        tracing::warn!("Cluster HTTP connection error: {:?}", err);
                    }
                });
            }
            _ = shutdown.cancelled() => {
                tracing::info!("[{}] Cluster server shutting down", node.config.node_id);
                return Ok(());
            }
        }
    }
}

#[cfg(test)]
#[path = "cluster_regressions.rs"]
mod regressions;
