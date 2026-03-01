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

// ============================================================================
// Types
// ============================================================================

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Role {
    Leader,
    Follower,
    Candidate,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogEntry {
    pub term: u64,
    pub index: u64,
    pub key: String,
    pub value: String,
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
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PutRequest {
    pub key: String,
    pub value: String,
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
        }
    }

    pub fn last_log_index(&self) -> u64 {
        self.log.last().map(|e| e.index).unwrap_or(0)
    }

    pub fn last_log_term(&self) -> u64 {
        self.log.last().map(|e| e.term).unwrap_or(0)
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
}

impl ClusterNode {
    pub fn new(config: ClusterConfig, db: sled::Db) -> Arc<Self> {
        let mut raft_state = RaftState::new();

        // Restore persisted state from sled
        if let Ok(Some(data)) = db.get("raft_meta") {
            if let Ok(meta) = serde_json::from_slice::<serde_json::Value>(&data) {
                raft_state.current_term = meta["current_term"].as_u64().unwrap_or(0);
                raft_state.voted_for = meta["voted_for"].as_str().map(|s| s.to_string());
            }
        }

        // Restore log entries
        if let Ok(log_tree) = db.open_tree("raft_log") {
            for item in log_tree.iter() {
                if let Ok((_key, val)) = item {
                    if let Ok(entry) = serde_json::from_slice::<LogEntry>(&val) {
                        raft_state.log.push(entry);
                    }
                }
            }
            raft_state.log.sort_by_key(|e| e.index);
        }

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

        if let Ok(Some(data)) = db.get("raft_peers") {
            if let Ok(persisted) =
                serde_json::from_slice::<HashMap<String, String>>(&data)
            {
                for (id, addr) in persisted {
                    if !raft_state.peer_addrs.values().any(|a| a == &addr)
                        && !raft_state.peers.contains(&addr)
                    {
                        raft_state.peers.push(addr.clone());
                    }
                    raft_state.peer_addrs.entry(id).or_insert(addr);
                }
            }
        }

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
        })
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

    fn persist_peers(&self, state: &RaftState) {
        let _ = self.db.insert(
            "raft_peers",
            serde_json::to_vec(&state.peer_addrs).unwrap().as_slice(),
        );
    }

    fn persist_meta(&self, state: &RaftState) {
        let meta = serde_json::json!({
            "current_term": state.current_term,
            "voted_for": state.voted_for,
        });
        let _ = self
            .db
            .insert("raft_meta", serde_json::to_vec(&meta).unwrap().as_slice());
    }

    fn persist_log_entry(&self, entry: &LogEntry) {
        if let Ok(log_tree) = self.db.open_tree("raft_log") {
            let key = entry.index.to_be_bytes();
            let _ = log_tree.insert(key, serde_json::to_vec(entry).unwrap().as_slice());
        }
    }

    fn truncate_log_from(&self, from_index: u64) {
        if let Ok(log_tree) = self.db.open_tree("raft_log") {
            // Remove entries with index >= from_index
            let start = from_index.to_be_bytes();
            for item in log_tree.range(start..) {
                if let Ok((key, _)) = item {
                    let _ = log_tree.remove(key);
                }
            }
        }
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
        let (term, last_log_index, last_log_term, peers) = {
            let mut state = self.state.write().await;
            state.current_term += 1;
            state.role = Role::Candidate;
            state.voted_for = Some(self.config.node_id.clone());
            state.leader_id = None;
            self.persist_meta(&state);
            (
                state.current_term,
                state.last_log_index(),
                state.last_log_term(),
                state.peers.clone(),
            )
        };

        let peer_count = peers.len();

        tracing::info!(
            "[{}] Starting election for term {}",
            self.config.node_id,
            term
        );

        let majority = (peer_count + 1) / 2 + 1;
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
                if resp.vote_granted {
                    votes += 1;
                }
                if resp.term > term {
                    let mut state = self.state.write().await;
                    state.current_term = resp.term;
                    state.role = Role::Follower;
                    state.voted_for = None;
                    self.persist_meta(&state);
                    return;
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
                self.persist_meta(&state);
                tracing::info!(
                    "[{}] Won election for term {} ({}/{} votes)",
                    self.config.node_id,
                    term,
                    votes,
                    peer_count + 1
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
        let (term, leader_id, prev_log_index, prev_log_term, entries, leader_commit, current_peer_addrs) = {
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
        };

        let url = format!("http://{}/raft/append-entries", peer);
        let resp = self.http_client.post(&url).json(&req).send().await;

        match resp {
            Ok(r) => {
                if let Ok(ae_resp) = r.json::<AppendEntriesResponse>().await {
                    let mut state = self.state.write().await;
                    if ae_resp.term > state.current_term {
                        state.current_term = ae_resp.term;
                        state.role = Role::Follower;
                        state.voted_for = None;
                        state.leader_id = None;
                        self.persist_meta(&state);
                        return;
                    }
                    if ae_resp.success {
                        if let Some(last) = entries.last() {
                            state.next_index.insert(peer.to_string(), last.index + 1);
                            state.match_index.insert(peer.to_string(), last.index);
                        }
                    } else {
                        // Decrement next_index and retry next heartbeat
                        let ni = state.next_index.get(peer).copied().unwrap_or(1);
                        if ni > 1 {
                            state.next_index.insert(peer.to_string(), ni - 1);
                        }
                    }
                }
            }
            Err(_) => {
                // Peer unreachable, will retry on next heartbeat
            }
        }
    }

    async fn update_commit_index(self: &Arc<Self>) {
        let mut state = self.state.write().await;
        if state.role != Role::Leader {
            return;
        }

        let peer_count = state.peers.len();
        let majority = (peer_count + 1) / 2 + 1;

        // Find the highest N such that a majority of match_index[i] >= N
        // and log[N].term == currentTerm
        let last_idx = state.last_log_index();
        for n in (state.commit_index + 1..=last_idx).rev() {
            let mut replication_count: usize = 1; // count self
            for peer in &state.peers.clone() {
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

        // Apply committed entries to the state machine
        while state.last_applied < state.commit_index {
            state.last_applied += 1;
            if let Some(entry) = state.log.get((state.last_applied - 1) as usize) {
                if let Ok(data_tree) = self.db.open_tree("data") {
                    let _ = data_tree.insert(entry.key.as_bytes(), entry.value.as_bytes());
                }
            }
        }
    }

    // --- RPC Handlers -------------------------------------------------------

    pub async fn handle_append_entries(&self, req: AppendEntriesRequest) -> AppendEntriesResponse {
        let mut state = self.state.write().await;

        // Reply false if term < currentTerm
        if req.term < state.current_term {
            return AppendEntriesResponse {
                term: state.current_term,
                success: false,
            };
        }

        // Update term if needed
        if req.term > state.current_term {
            state.current_term = req.term;
            state.voted_for = None;
        }

        state.role = Role::Follower;
        state.leader_id = Some(req.leader_id.clone());
        state.last_heartbeat = Instant::now();
        self.heartbeat_notify.notify_one();

        // Apply peer set from the leader if provided
        if let Some(leader_peer_addrs) = &req.peer_addrs {
            let mut changed = false;
            for (id, addr) in leader_peer_addrs {
                // Skip self
                if id == &self.config.node_id {
                    continue;
                }
                if !state.peer_addrs.contains_key(id) {
                    state.peers.push(addr.clone());
                    state.peer_addrs.insert(id.clone(), addr.clone());
                    changed = true;
                } else if state.peer_addrs.get(id) != Some(addr) {
                    // Address changed
                    if let Some(old_addr) = state.peer_addrs.insert(id.clone(), addr.clone()) {
                        state.peers.retain(|p| p != &old_addr);
                    }
                    state.peers.push(addr.clone());
                    changed = true;
                }
            }
            // Remove peers that the leader no longer has (excluding self)
            let leader_ids: std::collections::HashSet<&String> = leader_peer_addrs.keys().collect();
            let local_ids: Vec<String> = state.peer_addrs.keys().cloned().collect();
            for id in local_ids {
                if id != self.config.node_id && !leader_ids.contains(&id) {
                    if let Some(addr) = state.peer_addrs.remove(&id) {
                        state.peers.retain(|p| p != &addr);
                        changed = true;
                    }
                }
            }
            if changed {
                self.persist_peers(&state);
            }
        }

        // Log consistency check
        if req.prev_log_index > 0 {
            match state.log.get((req.prev_log_index - 1) as usize) {
                Some(entry) if entry.term != req.prev_log_term => {
                    // Conflicting entry, truncate
                    state.log.truncate((req.prev_log_index - 1) as usize);
                    self.truncate_log_from(req.prev_log_index);
                    self.persist_meta(&state);
                    return AppendEntriesResponse {
                        term: state.current_term,
                        success: false,
                    };
                }
                None if req.prev_log_index > state.log.len() as u64 => {
                    // Missing entries
                    self.persist_meta(&state);
                    return AppendEntriesResponse {
                        term: state.current_term,
                        success: false,
                    };
                }
                _ => {}
            }
        }

        // Append new entries
        for entry in &req.entries {
            let idx = (entry.index - 1) as usize;
            if idx < state.log.len() {
                if state.log[idx].term != entry.term {
                    state.log.truncate(idx);
                    self.truncate_log_from(entry.index);
                    state.log.push(entry.clone());
                    self.persist_log_entry(entry);
                }
            } else {
                state.log.push(entry.clone());
                self.persist_log_entry(entry);
            }
        }

        // Update commit index
        if req.leader_commit > state.commit_index {
            state.commit_index = std::cmp::min(req.leader_commit, state.last_log_index());
        }

        // Apply committed entries to state machine
        while state.last_applied < state.commit_index {
            state.last_applied += 1;
            if let Some(entry) = state.log.get((state.last_applied - 1) as usize) {
                if let Ok(data_tree) = self.db.open_tree("data") {
                    let _ = data_tree.insert(entry.key.as_bytes(), entry.value.as_bytes());
                }
            }
        }

        self.persist_meta(&state);
        AppendEntriesResponse {
            term: state.current_term,
            success: true,
        }
    }

    pub async fn handle_request_vote(&self, req: RequestVoteRequest) -> RequestVoteResponse {
        let mut state = self.state.write().await;

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
            self.persist_meta(&state);
            RequestVoteResponse {
                term: state.current_term,
                vote_granted: true,
            }
        } else {
            self.persist_meta(&state);
            RequestVoteResponse {
                term: state.current_term,
                vote_granted: false,
            }
        }
    }

    // --- Client Data Operations ---------------------------------------------

    /// Write a key-value pair. Must be called on the leader.
    pub async fn put(&self, key: String, value: String) -> Result<(), String> {
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
            };
            state.log.push(entry.clone());
            self.persist_log_entry(&entry);
            entry
        };

        // Wait for the entry to be committed (replicated to majority)
        let target_index = entry.index;
        for _ in 0..100 {
            tokio::select! {
                _ = self.commit_notify.notified() => {}
                _ = sleep(Duration::from_millis(50)) => {}
            }
            let state = self.state.read().await;
            if state.commit_index >= target_index {
                return Ok(());
            }
            if state.role != Role::Leader {
                return Err("lost leadership during replication".to_string());
            }
        }

        Err("timeout waiting for commit".to_string())
    }

    /// Write a key-value pair, forwarding to the leader if this node is a
    /// follower.  Uses the dynamic peer_addrs table to resolve the leader's
    /// network address from its node-id.
    pub async fn put_or_forward(&self, key: String, value: String) -> Result<(), String> {
        let (is_leader, leader_id, leader_addr) = {
            let state = self.state.read().await;
            let lid = state.leader_id.clone();
            let addr = lid
                .as_ref()
                .and_then(|id| state.peer_addrs.get(id).cloned());
            (state.role == Role::Leader, lid, addr)
        };

        if is_leader {
            return self.put(key, value).await;
        }

        // We are not the leader – try to forward.
        match leader_id {
            Some(id) => {
                let leader_addr = leader_addr
                    .ok_or_else(|| format!("unknown leader address for node '{}'", id))?;
                let url = format!("http://{}/data/put", leader_addr);
                let req = PutRequest {
                    key,
                    value,
                };
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

    // --- Dynamic Membership ------------------------------------------------

    /// Add a peer to the cluster.  Must be called on the leader.
    /// The peer is immediately added to the active peer set and persisted.
    pub async fn add_peer(&self, node_id: String, addr: String) -> Result<(), String> {
        let mut state = self.state.write().await;
        if state.role != Role::Leader {
            return Err(format!(
                "not the leader; current leader: {:?}",
                state.leader_id
            ));
        }

        // Idempotent: skip if already present
        if state.peer_addrs.contains_key(&node_id) {
            return Ok(());
        }

        state.peers.push(addr.clone());
        state.peer_addrs.insert(node_id.clone(), addr.clone());

        // Initialize replication state for the new peer
        let last_index = state.last_log_index();
        state.next_index.insert(addr.clone(), last_index + 1);
        state.match_index.insert(addr.clone(), 0);

        self.persist_peers(&state);
        tracing::info!("[{}] Added peer {}@{}", self.config.node_id, node_id, addr);
        Ok(())
    }

    /// Remove a peer from the cluster.  Must be called on the leader.
    pub async fn remove_peer(&self, node_id: String) -> Result<(), String> {
        let mut state = self.state.write().await;
        if state.role != Role::Leader {
            return Err(format!(
                "not the leader; current leader: {:?}",
                state.leader_id
            ));
        }

        if let Some(addr) = state.peer_addrs.remove(&node_id) {
            state.peers.retain(|p| p != &addr);
            state.next_index.remove(&addr);
            state.match_index.remove(&addr);
            self.persist_peers(&state);
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
        {
            let state = self.state.read().await;
            if state.role == Role::Leader {
                drop(state);
                return self.add_peer(req.node_id, req.addr).await;
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
                    // Also update our own peer set locally so we don't wait
                    // for the next heartbeat to discover the new peer.
                    let mut state = self.state.write().await;
                    if !state.peer_addrs.contains_key(&req.node_id) {
                        state.peers.push(req.addr.clone());
                        state.peer_addrs.insert(req.node_id, req.addr);
                        self.persist_peers(&state);
                    }
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
                    // Remove from local peer set too
                    let mut state = self.state.write().await;
                    if let Some(addr) = state.peer_addrs.remove(&req.node_id) {
                        state.peers.retain(|p| p != &addr);
                        self.persist_peers(&state);
                    }
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

    let response = match (method, path.as_str()) {
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
                Ok(put_req) => match node.put_or_forward(put_req.key, put_req.value).await {
                    Ok(()) => json_response(200, &serde_json::json!({"ok": true})),
                    Err(e) => error_response(503, &e),
                },
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

pub async fn start_cluster_server(node: Arc<ClusterNode>) -> Result<(), Box<dyn std::error::Error>>
{
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
