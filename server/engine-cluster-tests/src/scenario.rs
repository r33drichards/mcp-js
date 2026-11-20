//! Declarative, replayable cluster scenarios (milestones 1 and 2 of #316).
//!
//! A scenario is a serializable description of seed, topology, workload,
//! timed faults, recovery policy, and assertions. One runner executes it
//! against a real three-node Raft cluster under turmoil: JavaScript runs on
//! a real engine, the resulting label head is CAS'd through the cluster, and
//! an event log plus artifacts record exactly what happened so a failure
//! replays without fuzzing.
//!
//! Documented scope gaps: the topology is bounded to voters n0/n1/n2 plus
//! one observer client; storage faults and the sled/Turmoil filesystem
//! integration are separate milestones (#317), not covered here.
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use server::cluster::{
    AppendEntriesRequest, AppendEntriesResponse, CasRequest, CasResponse, ClusterConfig,
    ClusterNode, ForwardAck, JoinRequest, LeaveRequest, PeerRpc, PeerRpcFuture, PutRequest,
    RequestVoteRequest, RequestVoteResponse, Role,
};
use server::engine::Engine;
use server::engine::execution::ExecutionRegistry;
use server::engine::fs::FsConfig;
use server::engine::fs_store::FsStore;
use server::engine::fs_labels::{CaId, RefLogEntry};
use server::engine::fs_labels::LabelStore;
use server::engine::opa::{EvalMode, PolicyChain};
use server::engine::parse_ca_hex;
use std::{cell::RefCell, collections::BTreeMap, collections::HashMap, rc::Rc, sync::Arc, time::Duration};
use tokio::{io::{AsyncReadExt, AsyncWriteExt}, sync::watch, task::JoinSet};
use turmoil::net::{TcpListener, TcpStream};

const PORT: u16 = 4300;
type RpcFuture<'a, T> = PeerRpcFuture<'a, T>;

struct SimRpc;
impl PeerRpc for SimRpc {
    fn request_vote<'a>(&'a self, peer: &'a str, req: RequestVoteRequest) -> RpcFuture<'a, RequestVoteResponse> {
        Box::pin(async move { exchange(peer, "/raft/request-vote", &req).await })
    }
    fn append_entries<'a>(&'a self, peer: &'a str, req: AppendEntriesRequest) -> RpcFuture<'a, AppendEntriesResponse> {
        Box::pin(async move { exchange(peer, "/raft/append-entries", &req).await })
    }
    fn forward_put<'a>(&'a self, peer: &'a str, req: PutRequest) -> RpcFuture<'a, ForwardAck> {
        Box::pin(async move { exchange(peer, "/data/put", &req).await })
    }
    fn forward_cas<'a>(&'a self, peer: &'a str, req: CasRequest) -> RpcFuture<'a, CasResponse> {
        Box::pin(async move { exchange(peer, "/data/cas", &req).await })
    }
    fn forward_join<'a>(&'a self, peer: &'a str, req: JoinRequest) -> RpcFuture<'a, ForwardAck> {
        Box::pin(async move { exchange(peer, "/raft/join", &req).await })
    }
    fn forward_leave<'a>(&'a self, peer: &'a str, req: LeaveRequest) -> RpcFuture<'a, ForwardAck> {
        Box::pin(async move { exchange(peer, "/raft/leave", &req).await })
    }
}

async fn exchange<Req: Serialize, Resp: DeserializeOwned>(
    peer: &str,
    path: &str,
    req: &Req,
) -> Option<Resp> {
    exchange_with_timeout(peer, path, req, Duration::from_millis(150)).await
}

async fn exchange_with_timeout<Req: Serialize, Resp: DeserializeOwned>(
    peer: &str, path: &str, req: &Req, deadline: Duration,
) -> Option<Resp> {
    tokio::time::timeout(deadline, async {
        let (name, port) = peer.rsplit_once(':')?;
        let port: u16 = port.parse().ok()?;
        let mut stream = TcpStream::connect((name, port)).await.ok()?;
        let body = serde_json::to_vec(req).ok()?;
        let head = format!(
            "POST {path} HTTP/1.1\r\nHost: {name}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(head.as_bytes()).await.ok()?;
        stream.write_all(&body).await.ok()?;
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).await.ok()?;
        let raw = String::from_utf8(raw).ok()?;
        let (head, body) = raw.split_once("\r\n\r\n")?;
        if !head.starts_with("HTTP/1.1 200") { return None; }
        if let Some(line) = head.lines().find(|l| l.to_ascii_lowercase().starts_with("content-length:")) {
            let length: usize = line.split_once(':')?.1.trim().parse().ok()?;
            if body.len() != length { return None; }
        }
        serde_json::from_str(body).ok()
    }).await.ok().flatten()
}

pub fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// ---------------------------------------------------------------- scenario

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub struct Scenario {
    pub seed: u64,
    pub topology: Topology,
    pub workload: Workload,
    #[serde(default)]
    pub faults: Vec<Fault>,
    pub recovery: Recovery,
    pub asserts: Vec<Assertion>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Topology {
    pub voters: Vec<String>,
    pub clients: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Workload {
    pub every_ms: u64,
    pub count: usize,
    pub op: Op,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum Op {
    RunJs { node: String, program: Program },
    /// Broader JS track: no exact filesystem oracle is claimed.
    RunJsUnmodeled { node: String, source: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Program(pub Vec<Step>);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum Step {
    Write { path: String, bytes: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum Fault {
    Partition { at_ms: u64, groups: Vec<Vec<String>> },
    HealAll { at_ms: u64 },
    Crash { at_ms: u64, node: CrashTarget },
}

impl Fault {
    pub fn at_ms(&self) -> u64 {
        match self {
            Fault::Partition { at_ms, .. } | Fault::HealAll { at_ms } | Fault::Crash { at_ms, .. } => *at_ms,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
pub enum CrashTarget {
    Node(String),
    CurrentLeader,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recovery {
    pub stop_workload: bool,
    pub heal_all: bool,
    pub restart_crashed: bool,
    pub deadline_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Assertion {
    AckedCommitsInLegalHistory,
    ReplicasConvergeAfterRecovery,
    PublishedSnapshotMatchesModel,
    FreshWriteSucceeds,
    NoUnexpectedJsFailure,
}

impl Program {
    /// Render the steps into real JavaScript executed on the node; `{i}` is
    /// substituted with the iteration index.
    pub fn render_js(&self, i: usize) -> String {
        let mut body = String::new();
        for step in &self.0 {
            match step {
                Step::Write { path, bytes } => {
                    let bytes = bytes.replace("{i}", &i.to_string());
                    body.push_str(&format!(
                        "  await fs.writeFile(\"{path}\", \"{bytes}\");\n"
                    ));
                }
            }
        }
        format!("(async () => {{\n{body}}})()")
    }
}

impl Op {
    /// Resolve the dynamic `leader` selector against the recorded election.
    pub fn resolve_node(&self, leader: &str) -> String {
        match self {
            Op::RunJs { node, .. } if node == "leader" => leader.to_string(),
            Op::RunJs { node, .. } => node.clone(),
            Op::RunJsUnmodeled { node, .. } if node == "leader" => leader.to_string(),
            Op::RunJsUnmodeled { node, .. } => node.clone(),
        }
    }
}

impl Scenario {
    pub fn from_json(json: &str) -> Result<Self, String> {
        let scenario: Scenario = serde_json::from_str(json).map_err(|e| e.to_string())?;
        scenario.validate()?;
        Ok(scenario)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("scenario serializes")
    }

    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self).expect("scenario serializes")
    }

    /// Bounded topology: exactly the three fixed voters and one observer.
    fn validate(&self) -> Result<(), String> {
        if self.topology.voters != vec!["n0".to_string(), "n1".to_string(), "n2".to_string()] {
            return Err(format!(
                "unsupported topology: voters must be n0/n1/n2, got {:?}",
                self.topology.voters
            ));
        }
        if self.topology.clients != vec!["observer".to_string()] {
            return Err(format!(
                "unsupported topology: clients must be exactly [observer], got {:?}",
                self.topology.clients
            ));
        }
        if let Op::RunJsUnmodeled { source, .. } = &self.workload.op {
            if source.len() > 512 || self.workload.count == 0 || self.workload.count > 2 {
                return Err("unmodeled JS bounded to 512 source bytes and 1-2 executions".into());
            }
            if self.asserts.contains(&Assertion::PublishedSnapshotMatchesModel)
                || self.asserts.contains(&Assertion::FreshWriteSucceeds)
            {
                return Err("unmodeled JS has no exact filesystem/fresh-write oracle".into());
            }
            if !self.asserts.contains(&Assertion::NoUnexpectedJsFailure) {
                return Err("unmodeled JS requires the failure-classification oracle".into());
            }
        }
        if self.workload.count > 12 {
            return Err(format!("workload.count bounded to 12, got {}", self.workload.count));
        }
        Ok(())
    }

    /// Run the scenario, check its assertions, and persist artifacts.
    pub fn run_and_dump(&self, name: &str) -> ScenarioResult {
        let mut result = run_scenario(self);
        result.dump(self, name);
        result
    }
}

/// Replay a persisted scenario artifact; a failure replays without fuzzing.
pub fn replay_file(path: &str) -> Result<ScenarioResult, String> {
    let json = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let scenario = Scenario::from_json(&json)?;
    Ok(run_scenario(&scenario))
}

// ------------------------------------------------------------------ events

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum ScenarioEvent {
    Election { at_ms: u64, leader: String, term: u64 },
    Invoke { at_ms: u64, op_id: u64, node: String },
    Respond { at_ms: u64, op_id: u64, node: String, outcome: Outcome },
    Partition { at_ms: u64, a: String, b: String },
    Heal { at_ms: u64, a: String, b: String },
    HealAll { at_ms: u64 },
    Crash { at_ms: u64, node: String, target: String },
    Restart { at_ms: u64, node: String },
    Assert { at_ms: u64, name: String, passed: bool },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Outcome {
    Acked { head: String },
    Conflict,
    Failed { error: String },
    JsException { error: String },
    ResourceLimit { error: String },
    Unknown,
}

impl Outcome {
    pub fn kind(&self) -> &'static str {
        match self {
            Outcome::Acked { .. } => "acked",
            Outcome::Conflict => "conflict",
            Outcome::Failed { .. } => "failed",
            Outcome::JsException { .. } => "js_exception",
            Outcome::ResourceLimit { .. } => "resource_limit",
            Outcome::Unknown => "unknown",
        }
    }
}

// ------------------------------------------------------------------- model

/// Independent expected state: files that acknowledged operations wrote.
pub type Model = BTreeMap<String, Vec<u8>>;

// ------------------------------------------------------------------ runner

type Nodes = Rc<RefCell<HashMap<String, Arc<ClusterNode>>>>;
type Restarts = Rc<RefCell<HashMap<String, watch::Sender<()>>>>;

pub struct ScenarioResult {
    pub status: Result<String, String>,
    pub events: Vec<ScenarioEvent>,
    pub acked: Vec<String>,
    pub unknown: Vec<String>,
    pub failed: Vec<String>,
    pub final_heads: BTreeMap<String, Option<String>>,
    pub model: Model,
    pub artifact_dir: Option<String>,
}

impl ScenarioResult {
    pub fn expect_status(&self, expected: &str) {
        match &self.status {
            Ok(s) if s == expected => {},
            other => panic!(
                "scenario expected {expected:?}, got {other:?} (artifacts: {:?}) events={:?}",
                self.artifact_dir,
                self.events
            ),
        }
    }

    pub fn legal_history(&self) -> History {
        History {
            acked: self.acked.clone(),
            unknown: self.unknown.clone(),
            final_head: self.final_heads.values().next().cloned().flatten(),
        }
    }

    pub fn final_heads(&self) -> BTreeMap<String, Option<String>> {
        self.final_heads.clone()
    }

    pub fn model_snapshot(&self) -> Vec<(String, Vec<u8>)> {
        self.model.clone().into_iter().collect()
    }

    pub fn last_outcome(&self) -> Option<(&'static str, String)> {
        self.events.iter().rev().find_map(|e| match e {
            ScenarioEvent::Respond { outcome, .. } => Some((outcome.kind(), format!("{outcome:?}"))),
            _ => None,
        })
    }

    fn dump(&mut self, scenario: &Scenario, name: &str) {
        let dir = std::env::current_dir()
            .unwrap()
            .join("target/scenario-artifacts")
            .join(name);
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join("scenario.json"), scenario.to_json_pretty());
        let _ = std::fs::write(
            dir.join("events.json"),
            serde_json::to_string_pretty(&self.events).unwrap_or_default(),
        );
        let summary = serde_json::json!({
            "status": format!("{:?}", self.status),
            "acked": self.acked,
            "unknown": self.unknown,
            "failed": self.failed,
            "final_heads": self.final_heads,
        });
        let _ = std::fs::write(dir.join("result.json"), serde_json::to_string_pretty(&summary).unwrap_or_default());
        self.artifact_dir = Some(dir.to_string_lossy().into_owned());
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct History {
    pub acked: Vec<String>,
    pub unknown: Vec<String>,
    pub final_head: Option<String>,
}

fn now_ms(start: tokio::time::Instant) -> u64 {
    start.elapsed().as_millis() as u64
}


fn engine_with_cluster(node: Arc<ClusterNode>, tag: &str) -> (Arc<Engine>, Arc<FsStore>) {
    let registry = ExecutionRegistry::new(
        std::env::temp_dir()
            .join(format!("mcp-scenario-eng-{tag}-{}-{}", std::process::id(), RUN_ID.with(|c| c.borrow().clone())))
            .to_str()
            .unwrap(),
    )
    .expect("registry");
    let fs_config = FsConfig::new(Arc::new(PolicyChain::new(vec![], EvalMode::All)));
    let store = Arc::new(FsStore::in_memory());
    let labels = LabelStore::in_memory().with_cluster(node);
    let engine = Engine::from_engine(
        Engine::new_stateless(64 * 1024 * 1024, 30, 4)
            .with_fs_config(fs_config)
            .with_execution_registry(Arc::new(registry))
            .with_fs_snapshots(store.clone(), Arc::new(labels)),
    );
    (engine, store)
}

/// Build an engine entirely within the simulated host that owns its sled DB.
/// The returned pump must be dropped with that host incarnation.
#[cfg(feature = "real-sled")]
pub fn engine_with_cluster_host(
    node: Arc<ClusterNode>,
    db: sled::Db,
) -> (Arc<Engine>, Arc<FsStore>, tokio::task::JoinHandle<()>, tokio::task::JoinHandle<()>, Arc<server::engine::console::HostConsoleBridge>) {
    use server::engine::heap_storage::{HostBlobStorage, SledHeapStorage};
    use server::engine::console::HostConsoleBridge;

    let registry = Arc::new(ExecutionRegistry::from_db(db.clone()));
    let (console_bridge, console_pump) = HostConsoleBridge::new(registry.clone());
    let console_pump = tokio::spawn(console_pump);
    let labels = LabelStore::from_db(db.clone()).with_cluster(node);
    let storage = Arc::new(SledHeapStorage::new(db).expect("host blob tree"));
    let (worker_storage, pump) = HostBlobStorage::bridge(storage);
    let pump = tokio::spawn(pump);
    let store = Arc::new(FsStore::new(Arc::new(worker_storage)));
    let fs_config = FsConfig::new(Arc::new(PolicyChain::new(vec![], EvalMode::All)));
    let engine = Engine::from_engine(
        Engine::new_stateless(64 * 1024 * 1024, 30, 4)
            .with_fs_config(fs_config)
            .with_execution_registry(registry)
            .with_host_console_bridge(console_bridge.clone())
            .with_fs_snapshots(store.clone(), Arc::new(labels)),
    );
    (engine, store, pump, console_pump, console_bridge)
}

#[cfg(feature = "real-sled")]
const HOST_PORT: u16 = 4301;

#[cfg(feature = "real-sled")]
#[derive(Serialize, Deserialize)]
enum HostRequest {
    Status,
    Generation,
    RunJs { code: String },
    ReadFile { head: String, path: String },
    Resolve { label: String },
    Cas { label: String, expect: Option<CaId>, new: CaId },
    Log { label: String },
}

#[cfg(feature = "real-sled")]
#[derive(Serialize, Deserialize)]
enum HostResponse {
    Status(Role, u64),
    Generation(u64),
    RunJs(JsResult),
    ReadFile(Result<Option<Vec<u8>>, String>),
    Resolve(Result<Option<CaId>, String>),
    Cas(Result<bool, String>),
    Log(Result<Vec<RefLogEntry>, String>),
}

/// All handles, including the sled-backed label and blob stores, stay on the
/// owning simulated host. Observer clients exchange only serialized messages.
#[cfg(feature = "real-sled")]
pub struct ScenarioHost {
    node: Arc<ClusterNode>,
    engine: Arc<Engine>,
    store: Arc<FsStore>,
    generation: u64,
    _pump: tokio::task::JoinHandle<()>,
    _console_pump: tokio::task::JoinHandle<()>,
    console_bridge: Arc<server::engine::console::HostConsoleBridge>,
}

#[cfg(feature = "real-sled")]
impl ScenarioHost {
    pub fn new(node: Arc<ClusterNode>, db: sled::Db) -> Self {
        Self::new_with_generation(node, db, 1)
    }

    pub fn new_with_generation(node: Arc<ClusterNode>, db: sled::Db, generation: u64) -> Self {
        let (engine, store, pump, console_pump, console_bridge) = engine_with_cluster_host(node.clone(), db);
        Self {
            node,
            engine,
            store,
            generation,
            _pump: pump,
            _console_pump: console_pump,
            console_bridge,
        }
    }

    /// Diagnostic handle for verifying the owning host stops its blob pump.
    pub fn pump_abort_handle(&self) -> tokio::task::AbortHandle {
        self._pump.abort_handle()
    }

    /// Handle to the host console bridge, for tests that must capture a
    /// console writer before a crash and probe it afterwards.
    pub fn console_bridge(&self) -> Arc<server::engine::console::HostConsoleBridge> {
        self.console_bridge.clone()
    }

    /// Serve one request and close the connection. A host loop owns this value.
    pub async fn serve_request(&self, mut stream: TcpStream) -> Result<(), String> {
        let mut raw = Vec::new();
        let (offset, length) = loop {
            let mut chunk = [0u8; 4096];
            let n = stream.read(&mut chunk).await.map_err(|e| e.to_string())?;
            if n == 0 {
                return Err("request ended before body".into());
            }
            raw.extend_from_slice(&chunk[..n]);
            if raw.len() > 1024 * 1024 {
                return Err("request too large".into());
            }
            if let Some(end) = raw.windows(4).position(|x| x == b"\r\n\r\n") {
                let header = std::str::from_utf8(&raw[..end]).map_err(|e| e.to_string())?;
                if !header.starts_with("POST /scenario HTTP/1.1") {
                    return Err("unexpected request path".into());
                }
                let length = header
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("Content-Length")
                            .then(|| value.trim().parse::<usize>().ok())
                            .flatten()
                    })
                    .ok_or("missing content length")?;
                let offset = end + 4;
                if raw.len() >= offset + length {
                    break (offset, length);
                }
            }
        };
        let request: HostRequest =
            serde_json::from_slice(&raw[offset..offset + length]).map_err(|e| e.to_string())?;
        let response = match request {
            HostRequest::Status => {
                let status = self.node.status().await;
                HostResponse::Status(status.role, status.term)
            }
            HostRequest::Generation => HostResponse::Generation(self.generation),
            HostRequest::RunJs { code } => HostResponse::RunJs(run_js(&self.engine, &code).await),
            HostRequest::ReadFile { head, path } => {
                HostResponse::ReadFile(read_file_at_checked(&self.store, &head, &path).await)
            }
            HostRequest::Resolve { label } => {
                let labels = self.engine.label_store().ok_or("host label store missing")?;
                HostResponse::Resolve(labels.resolve(&label).await)
            }
            HostRequest::Cas { label, expect, new } => {
                let labels = self.engine.label_store().ok_or("host label store missing")?;
                HostResponse::Cas(labels.cas(&label, expect, new, None).await)
            }
            HostRequest::Log { label } => {
                let labels = self.engine.label_store().ok_or("host label store missing")?;
                HostResponse::Log(labels.log(&label).await)
            }
        };
        let body = serde_json::to_vec(&response).map_err(|e| e.to_string())?;
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream
            .write_all(header.as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        stream.write_all(&body).await.map_err(|e| e.to_string())?;
        Ok(())
    }
}

#[cfg(feature = "real-sled")]
pub struct ScenarioHostClient {
    peer: String,
}

#[cfg(feature = "real-sled")]
impl ScenarioHostClient {
    pub fn new(name: &str) -> Self {
        Self {
            peer: format!("{name}:{HOST_PORT}"),
        }
    }

    async fn call(&self, request: HostRequest, deadline: Duration) -> Result<HostResponse, String> {
        exchange_with_timeout(&self.peer, "/scenario", &request, deadline)
            .await
            .ok_or_else(|| format!("host {} unavailable", self.peer))
    }

    pub async fn status(&self) -> Result<(Role, u64), String> {
        match self
            .call(HostRequest::Status, Duration::from_secs(2))
            .await?
        {
            HostResponse::Status(role, term) => Ok((role, term)),
            _ => Err("unexpected host status response".into()),
        }
    }

    pub async fn generation(&self) -> Result<u64, String> {
        match self.call(HostRequest::Generation, Duration::from_secs(2)).await? {
            HostResponse::Generation(generation) => Ok(generation),
            _ => Err("unexpected host generation response".into()),
        }
    }

    pub async fn run_js(&self, code: &str) -> Result<JsResult, String> {
        match self
            .call(
                HostRequest::RunJs { code: code.into() },
                Duration::from_secs(35),
            )
            .await?
        {
            HostResponse::RunJs(result) => Ok(result),
            _ => Err("unexpected host JS response".into()),
        }
    }

    pub async fn resolve(&self, label: &str) -> Result<Option<CaId>, String> {
        match self.call(HostRequest::Resolve { label: label.into() }, Duration::from_secs(2)).await? {
            HostResponse::Resolve(result) => result,
            _ => Err("unexpected host resolve response".into()),
        }
    }

    pub async fn cas(&self, label: &str, expect: Option<CaId>, new: CaId) -> Result<bool, String> {
        let request = HostRequest::Cas { label: label.into(), expect, new };
        match self.call(request, Duration::from_secs(5)).await? {
            HostResponse::Cas(result) => result,
            _ => Err("unexpected host CAS response".into()),
        }
    }

    pub async fn log(&self, label: &str) -> Result<Vec<RefLogEntry>, String> {
        match self.call(HostRequest::Log { label: label.into() }, Duration::from_secs(2)).await? {
            HostResponse::Log(result) => result,
            _ => Err("unexpected host log response".into()),
        }
    }

    pub async fn read_file(&self, head: &str, path: &str) -> Result<Option<Vec<u8>>, String> {
        let request = HostRequest::ReadFile {
            head: head.into(),
            path: path.into(),
        };
        match self.call(request, Duration::from_secs(2)).await? {
            HostResponse::ReadFile(result) => result,
            _ => Err("unexpected host file response".into()),
        }
    }
}

// The generated program catches only pure-JS exceptions. Native filesystem and
// engine failures are outside that catch and must not be text-classified.
const JS_EXCEPTION_MARKER: &str = "__mcp_js_expected_exception__";

#[derive(Debug, Serialize, Deserialize)]
pub enum JsResult {
    Head(String),
    JsException(String),
    Failed(String),
    TimedOut,
}

async fn run_js(engine: &Engine, code: &str) -> JsResult {
    let exec_id = match engine
        .run_js(code.to_string())
        .maybe_fs(Some("main".to_string()))
        .execute()
        .await
    {
        Ok(id) => id,
        Err(e) => return JsResult::Failed(format!("execution submission failed: {e}")),
    };
    for _ in 0..1200 {
        tokio::time::sleep(Duration::from_millis(25)).await;
        if let Ok(info) = engine.get_execution(exec_id.clone()) {
            match info.status.as_str() {
                "completed" => {
                    if let Some(result) = &info.result {
                        // Only a complete generated exception tag qualifies; ordinary
                        // output mentioning the marker is still an ordinary result.
                        if matches!(
                            result.strip_prefix(JS_EXCEPTION_MARKER),
                            Some("Error" | "TypeError" | "RangeError" | "ReferenceError" |
                                 "SyntaxError" | "URIError" | "EvalError" | "AggregateError")
                        ) {
                            return JsResult::JsException(result.clone());
                        }
                    }
                    return match info.fs {
                        Some(head) => JsResult::Head(head),
                        None => JsResult::Failed("completed without fs head".into()),
                    };
                }
                "failed" => {
                    return JsResult::Failed(info.error.unwrap_or_else(|| "failed".into()));
                }
                "timed_out" => return JsResult::TimedOut,
                _ => continue,
            }
        }
    }
    JsResult::Failed("execution status unavailable after polling deadline".into())
}

async fn read_file_at(store: &FsStore, head_hex: &str, path: &str) -> Option<Vec<u8>> {
    let ca: CaId = parse_ca_hex(head_hex)?;
    let comps: Vec<String> = path
        .trim_start_matches('/')
        .split('/')
        .map(str::to_string)
        .collect();
    let entry = store.resolve_file(Some(ca), &comps).await.ok()??;
    store.read_file(&entry).await.ok()
}

#[cfg(feature = "real-sled")]
async fn read_file_at_checked(store: &FsStore, head_hex: &str, path: &str) -> Result<Option<Vec<u8>>, String> {
    let ca = parse_ca_hex(head_hex)
        .ok_or_else(|| format!("invalid snapshot head: {head_hex}"))?;
    let comps: Vec<String> = path
        .trim_start_matches('/')
        .split('/')
        .map(str::to_string)
        .collect();
    let Some(entry) = store.resolve_file(Some(ca), &comps).await.map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    store.read_file(&entry).await.map(Some).map_err(|e| e.to_string())
}

async fn current_leader(nodes: &HashMap<String, Arc<ClusterNode>>) -> Option<String> {
    for name in ["n0", "n1", "n2"] {
        if let Some(n) = nodes.get(name) {
            if n.status().await.role == Role::Leader {
                return Some(name.to_string());
            }
        }
    }
    None
}

const HOSTS: [&str; 4] = ["n0", "n1", "n2", "observer"];

fn blank(status: Err2) -> ScenarioResult { ScenarioResult { status, events: Vec::new(), acked: Vec::new(), unknown: Vec::new(), failed: Vec::new(), final_heads: BTreeMap::new(), model: BTreeMap::new(), artifact_dir: None } }
type Err2 = Result<String, String>;

#[cfg(feature = "real-sled")]
#[path = "scenario_real.rs"]
mod real;

#[cfg(feature = "real-sled")]
fn run_scenario(scenario: &Scenario) -> ScenarioResult { real::run_scenario(scenario) }

/// Ordinary sled control: the original scenario runner remains unchanged.
#[cfg(not(feature = "real-sled"))]
fn run_scenario(scenario: &Scenario) -> ScenarioResult {
    RUN_ID.with(|c| {
        *c.borrow_mut() = format!("{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
    });
    let shared: Nodes = Rc::new(RefCell::new(HashMap::new()));
    let restarts: Restarts = Rc::new(RefCell::new(HashMap::new()));
    let max_fault = scenario.faults.iter().map(|f| f.at_ms()).max().unwrap_or(0);
    let total_secs = 60 + (max_fault / 1000) + (scenario.recovery.deadline_ms / 1000);
    let mut sim = turmoil::Builder::new()
        .rng_seed(scenario.seed)
        .enable_random_order()
        .min_message_latency(Duration::from_millis(1))
        .max_message_latency(Duration::from_millis(3))
        .simulation_duration(Duration::from_secs(total_secs))
        .build();
    for (i, name) in ["n0", "n1", "n2"].into_iter().enumerate() {
        let peers: Vec<_> = ["n0", "n1", "n2"]
            .into_iter()
            .filter(|p| *p != name)
            .map(|p| format!("{p}:{PORT}"))
            .collect();
        let peer_addrs = ["n0", "n1", "n2"]
            .into_iter()
            .filter(|p| *p != name)
            .map(|p| (p.to_owned(), format!("{p}:{PORT}")))
            .collect();
        let timeout = match i {
            0 => 70,
            1 => 180,
            _ => 300,
        };
        let config = ClusterConfig {
            node_id: name.to_owned(),
            peers,
            peer_addrs,
            cluster_port: PORT,
            advertise_addr: Some(format!("{name}:{PORT}")),
            heartbeat_interval: Duration::from_millis(20),
            election_timeout_min: Duration::from_millis(timeout),
            election_timeout_max: Duration::from_millis(timeout),
            learner: false,
        };
        let shared = shared.clone();
        let restart_tx = restarts.clone();
        sim.host(name, move || {
            let shared = shared.clone();
            let config = config.clone();
            let mut restarts_map = restart_tx.borrow_mut();
            let (tx, rx) = watch::channel(());
            restarts_map.insert(name.to_owned(), tx);
            drop(restarts_map);
            let mut rx = rx;
            async move {
                loop {
                    // Same restart pattern as the peer simulator: reopen the
                    // durable sled path so committed state survives crashes.
                    let db_path = std::env::temp_dir().join(format!(
                        "mcp-scenario-sled-{name}-{}-{}",
                        std::process::id(),
                        RUN_ID.with(|c| c.borrow().clone())
                    ));
                    let db = {
                        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
                        let mut reported = false;
                        loop {
                            let config = sled::Config::new().path(&db_path);
                            #[cfg(feature = "real-sled")]
                            let config = config.flush_every_ms(None);
                            match config.open() {
                                Ok(db) => break db,
                                Err(e) if tokio::time::Instant::now() < deadline => {
                                    if !reported {
                                        eprintln!("[SCENARIO_IO] {name} path={db_path:?} first sled open error: {e}");
                                        reported = true;
                                    }
                                    tokio::time::sleep(Duration::from_millis(10)).await;
                                    let _ = e;
                                }
                                Err(e) => return Err(e.into()),
                            }
                        }
                    };
                    let node = ClusterNode::new(config.clone(), db).with_peer_rpc(Arc::new(SimRpc));
                    let listener = TcpListener::bind(("0.0.0.0", PORT)).await?;
                    shared
                        .borrow_mut()
                        .insert(node.status().await.node_id.clone(), node.clone());
                    node.start_background_tasks().expect("start election and heartbeat loops");
                    let mut connections = JoinSet::new();
                    loop {
                        tokio::select! {
                            incoming = listener.accept() => {
                                let (stream, _) = incoming?;
                                let node = node.clone();
                                connections.spawn(async move {
                                    server::cluster::serve_cluster_connection(node, stream).await
                                });
                            }
                            done = connections.join_next(), if !connections.is_empty() => {
                                if let Some(Err(panic)) = done {
                                    panic!("cluster handler panicked: {panic}");
                                }
                            }
                            _ = rx.changed() => {
                                // Crash: stop everything this host is running.
                                node.shutdown();
                                connections.abort_all();
                                drop(listener);
                                shared.borrow_mut().remove(&name.to_owned());
                                tokio::time::sleep(Duration::from_millis(5)).await;
                                break;
                            }
                        }
                    }
                }
            }
        });
    }
    let sc = scenario.clone();
    sim.client("observer", async move { run_client(sc, shared, restarts).await });
    if let Err(e) = sim.run() {
        return blank(Err(format!("turmoil simulation failed: {e}")));
    }
    take_result()
}

thread_local! {
    static LAST_RESULT: RefCell<Option<ScenarioResult>> = const { RefCell::new(None) };
    static RUN_ID: RefCell<String> = RefCell::new(String::new());
}

fn store_result(r: ScenarioResult) {
    LAST_RESULT.with(|c| *c.borrow_mut() = Some(r));
}

fn take_result() -> ScenarioResult {
    LAST_RESULT
        .with(|c| c.borrow_mut().take())
        .unwrap_or_else(|| blank(Err("scenario client never completed".into())))
}

async fn run_client(sc: Scenario, shared: Nodes, restarts: Restarts) -> turmoil::Result {
    let start = tokio::time::Instant::now();
    let mut events: Vec<ScenarioEvent> = Vec::new();
    let mut acked: Vec<String> = Vec::new();
    let mut unknown: Vec<String> = Vec::new();
    let mut failed: Vec<String> = Vec::new();
    let mut model: Model = BTreeMap::new();
    let mut crashed: Vec<String> = Vec::new();
    let mut attempted: Vec<String> = Vec::new();

    // Election boundary: wait for a leader and record it.
    tokio::time::sleep(Duration::from_millis(600)).await;
    let nodes = shared.borrow().clone();
    let mut leader = String::new();
    let mut term = 0u64;
    for name in ["n0", "n1", "n2"] {
        if let Some(n) = nodes.get(name) {
            let s = n.status().await;
            if s.role == Role::Leader {
                leader = name.to_string();
                term = s.term;
            }
        }
    }
    if leader.is_empty() {
        store_result(blank(Err("no leader elected within 600ms".into())));
        return Ok(());
    }
    events.push(ScenarioEvent::Election { at_ms: now_ms(start), leader: leader.clone(), term });
    let follower = ["n0", "n1", "n2"]
        .into_iter()
        .find(|n| *n != leader)
        .unwrap()
        .to_string();
    let observer_store =
        Arc::new(LabelStore::in_memory().with_cluster(nodes[&follower].clone()));
    let op_node = sc.workload.op.resolve_node(&leader);
    let (op_engine, fs_store) = match nodes.get(&op_node) {
        Some(n) => engine_with_cluster(n.clone(), "op"),
        None => {
            store_result(blank(Err(format!("workload node {op_node} missing"))));
            return Ok(());
        }
    };
    let mut op_start = start + Duration::from_millis(600);
    let mut faults: Vec<Fault> = sc.faults.clone();
    faults.sort_by_key(|f| f.at_ms());
    let mut ops_done = 0usize;
    let mut op_id = 0u64;

    // Single scheduler loop: faults and operations interleave by sim time.
    loop {
        let next_fault = faults.first().map(|f| f.at_ms());
        let next_op = if ops_done < sc.workload.count { Some(op_start) } else { None };
        let fault_first = match (next_fault, next_op) {
            (None, None) => break,
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (Some(ft), Some(ot)) => ft <= ot.duration_since(start).as_millis() as u64,
        };
        if fault_first {
            let fault = faults.remove(0);
            // Faults fire at their scheduled sim time, not as soon as the
            // scheduler reaches them.
            tokio::time::sleep_until(start + Duration::from_millis(fault.at_ms())).await;
            let at = now_ms(start);
            match &fault {
                Fault::Partition { groups, .. } => {
                    // Dynamic selectors resolve at this boundary and are
                    // recorded; the resolved leader keeps only its own group
                    // so the partition is consistent.
                    let leader_group = groups.iter().position(|g| g.iter().any(|h| h == "current_leader"));
                    let mut resolved: Vec<Vec<String>> = Vec::new();
                    for (gi, g) in groups.iter().enumerate() {
                        let mut rg: Vec<String> = Vec::new();
                        for host in g {
                            let h = if host == "current_leader" { leader.clone() } else { host.clone() };
                            // Suppress duplicate leader names only when a dynamic
                            // selector chose its group. Explicit names must work too.
                            if leader_group.is_some_and(|chosen| h == leader && gi != chosen) { continue; }
                            if !rg.contains(&h) { rg.push(h); }
                        }
                        resolved.push(rg);
                    }
                    // Only hosts the scenario names take part: an unlisted
                    // host keeps its links, so the observer is isolated only
                    // when the scenario says so.
                    let named: Vec<&str> =
                        resolved.iter().flatten().map(|s| s.as_str()).collect();
                    let group_of = |h: &str| resolved.iter().position(|g| g.iter().any(|x| x == h));
                    for a in 0..HOSTS.len() {
                        for b in a + 1..HOSTS.len() {
                            let (ha, hb) = (HOSTS[a], HOSTS[b]);
                            if named.contains(&ha) && named.contains(&hb) && group_of(ha) != group_of(hb) {
                                turmoil::partition(ha, hb);
                                events.push(ScenarioEvent::Partition { at_ms: at, a: ha.into(), b: hb.into() });
                            }
                        }
                    }
                }
                Fault::HealAll { .. } => {
                    for a in 0..HOSTS.len() {
                        for b in a + 1..HOSTS.len() {
                            turmoil::repair(HOSTS[a], HOSTS[b]);
                        }
                    }
                    events.push(ScenarioEvent::HealAll { at_ms: at });
                }
                Fault::Crash { node, .. } => {
                    let target = match node {
                        CrashTarget::Node(n) => n.clone(),
                        CrashTarget::CurrentLeader => {
                            current_leader(&nodes).await.unwrap_or_else(|| leader.clone())
                        }
                    };
                    if let Some(n) = nodes.get(&target) {
                        n.shutdown();
                        crashed.push(target.clone());
                        events.push(ScenarioEvent::Crash { at_ms: at, node: target.clone(), target: format!("{node:?}={target}") });
                    }
                }
            }
        } else {
            tokio::time::sleep_until(op_start).await;
            let i = ops_done;
            op_id += 1;
            let node = sc.workload.op.resolve_node(&leader);
            events.push(ScenarioEvent::Invoke { at_ms: now_ms(start), op_id, node: node.clone() });
            let code = match &sc.workload.op {
                Op::RunJs { program, .. } => program.render_js(i),
                Op::RunJsUnmodeled { source, .. } => source.clone(),
            };
            let head = run_js(&op_engine, &code).await;
            let unmodeled = matches!(sc.workload.op, Op::RunJsUnmodeled { .. });
            let outcome = match head {
                JsResult::TimedOut if unmodeled => Outcome::ResourceLimit { error: "engine timed out".into() },
                JsResult::TimedOut => {
                    failed.push(format!("op{op_id}:timed_out"));
                    Outcome::Failed { error: "timed_out".into() }
                }
                JsResult::JsException(e) if unmodeled => Outcome::JsException { error: e },
                JsResult::JsException(e) => {
                    failed.push(format!("op{op_id}:{e}"));
                    Outcome::Failed { error: e }
                }
                JsResult::Failed(e) => {
                    failed.push(format!("op{op_id}:{e}"));
                    Outcome::Failed { error: e }
                }
                JsResult::Head(h) => {
                    attempted.push(h.clone());
                    let Some(ca) = parse_ca_hex(&h) else { failed.push(format!("op{op_id}:bad-ca-hex")); ops_done += 1; op_start = tokio::time::Instant::now() + Duration::from_millis(sc.workload.every_ms); continue; };
                    let expect = observer_store.resolve("main").await.ok().flatten();
                    match observer_store.cas("main", expect, ca, None).await {
                        Ok(true) => {
                            acked.push(h.clone());
                            if let Op::RunJs { program, .. } = &sc.workload.op {
                                for step in &program.0 {
                                    if let Step::Write { path, bytes } = step {
                                        model.insert(path.clone(), bytes.replace("{i}", &i.to_string()).into_bytes());
                                    }
                                }
                            }
                            Outcome::Acked { head: h }
                        }
                        Ok(false) => Outcome::Conflict,
                        Err(_) => {
                            unknown.push(format!("op{op_id}"));
                            Outcome::Unknown
                        }
                    }
                }
            };
            events.push(ScenarioEvent::Respond { at_ms: now_ms(start), op_id, node, outcome });
            ops_done += 1;
            op_start = tokio::time::Instant::now() + Duration::from_millis(sc.workload.every_ms);
        }
    }

    // Recovery: heal, restart crashed nodes, then bounded convergence.
    if sc.recovery.heal_all {
        for a in 0..HOSTS.len() {
            for b in a + 1..HOSTS.len() {
                turmoil::repair(HOSTS[a], HOSTS[b]);
            }
        }
        events.push(ScenarioEvent::HealAll { at_ms: now_ms(start) });
    }
    if sc.recovery.restart_crashed {
        for node in crashed.clone() {
            if let Some(tx) = restarts.borrow().get(&node) {
                let _ = tx.send(());
            }
            // Wait for the new incarnation to register in the shared map.
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            loop {
                if shared.borrow().contains_key(&node) { break; }
                assert!(tokio::time::Instant::now() < deadline, "{node}: restart never came up");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            events.push(ScenarioEvent::Restart { at_ms: now_ms(start), node });
        }
    }
    // A pre-fault acknowledgment is not post-recovery progress. A leader may
    // still be re-electing when links heal, so retry only uncertain/conflicting
    // publishes within the recovery budget. Every retry has a distinct JS
    // write and operation ID; no timeout is interpreted as non-commit.
    let mut fresh_write_acked = false;
    let mut fresh_attempted_iteration = None;
    if sc.asserts.contains(&Assertion::FreshWriteSucceeds) {
        let target = sc.workload.op.resolve_node(&leader);
        let deadline = tokio::time::Instant::now()
            + Duration::from_millis(sc.recovery.deadline_ms.min(3000));
        for retry in 0..5 {
            if retry > 0 {
                tokio::time::sleep(Duration::from_millis(200)).await;
                if tokio::time::Instant::now() >= deadline { break; }
            }
            let iteration = sc.workload.count + retry;
            op_id += 1;
            events.push(ScenarioEvent::Invoke { at_ms: now_ms(start), op_id, node: target.clone() });
            let mut fresh_head_advanced = false;
            let outcome = if crashed.iter().any(|n| n == &target || n == &follower) {
                // Old handles are not generation-fenced after a crash. Fail
                // closed until host-owned fresh incarnations are implemented.
                Outcome::Failed { error: "fresh JS probe after a crashed host requires host-local reopen".into() }
            } else if let Op::RunJs { program, .. } = &sc.workload.op {
                match run_js(&op_engine, &program.render_js(iteration)).await {
                    JsResult::Head(head) => {
                        fresh_attempted_iteration = Some(iteration);
                        attempted.push(head.clone());
                        match parse_ca_hex(&head) {
                            None => Outcome::Failed { error: "fresh JS returned invalid head".into() },
                            Some(ca) => match observer_store.resolve("main").await {
                                Err(_) => Outcome::Unknown,
                                Ok(expect) => {
                                    fresh_head_advanced = expect.as_ref() != Some(&ca);
                                    match observer_store.cas("main", expect, ca, None).await {
                                        Ok(true) => Outcome::Acked { head },
                                        Ok(false) => Outcome::Conflict,
                                        Err(_) => Outcome::Unknown,
                                    }
                                },
                            },
                        }
                    }
                    JsResult::Failed(error) | JsResult::JsException(error) => Outcome::Failed { error },
                    JsResult::TimedOut => Outcome::Failed { error: "fresh JS timed out".into() },
                }
            } else {
                Outcome::Failed { error: "fresh write requires a modeled JS program".into() }
            };
            let terminal_failure = matches!(&outcome, Outcome::Failed { .. });
            match &outcome {
                Outcome::Acked { head } => {
                    // An acknowledged CAS of the existing head is a no-op,
                    // not a fresh snapshot publication.
                    fresh_write_acked = fresh_head_advanced;
                    acked.push(head.clone());
                    if let Op::RunJs { program, .. } = &sc.workload.op {
                        for step in &program.0 {
                            let Step::Write { path, bytes } = step;
                            model.insert(path.clone(), bytes.replace("{i}", &iteration.to_string()).into_bytes());
                        }
                    }
                }
                Outcome::Unknown => unknown.push(format!("op{op_id}")),
                Outcome::Failed { error } => failed.push(format!("op{op_id}:{error}")),
                _ => {}
            }
            events.push(ScenarioEvent::Respond { at_ms: now_ms(start), op_id, node: target.clone(), outcome });
            if fresh_write_acked || terminal_failure { break; }
        }
    }

    // Convergence: every node's head must reach a value in the legal target
    // set: the last acked head, or the head of a final unresolved operation
    // (an unknown outcome may have committed; it is only reconciled by
    // observation, never assumed).
    let nodes = shared.borrow().clone();
    let mut final_heads: BTreeMap<String, Option<String>> = BTreeMap::new();
    let mut no_convergence: Option<String> = None;
    let deadline = tokio::time::Instant::now() + Duration::from_millis(sc.recovery.deadline_ms);
    loop {
        final_heads.clear();
        for name in ["n0", "n1", "n2"] {
            let head = match nodes.get(name) {
                Some(n) => {
                    let s = LabelStore::in_memory().with_cluster(n.clone());
                    s.resolve("main").await.ok().flatten().map(|c| to_hex(&c))
                }
                None => None,
            };
            final_heads.insert(name.to_string(), head);
        }
        let heads: Vec<Option<String>> = final_heads.values().cloned().collect();
        let first = heads.first().cloned().flatten();
        let all_equal = heads.iter().all(|h| h.as_ref() == first.as_ref());
        // Legal final head: nothing published, or any head an operation
        // actually produced (an unknown outcome may have committed; it is
        // reconciled by observation, never assumed).
        let legal = first.is_none()
            || attempted.iter().any(|h| Some(h.as_str()) == first.as_deref());
        if all_equal && legal {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            // Record and continue: the event log and artifacts must survive a
            // failed convergence so the scenario replays and diagnoses.
            no_convergence = Some(format!(
                "replicas did not converge within recovery deadline: {final_heads:?} acked={acked:?} attempted={attempted:?} unknown={unknown:?}"
            ));
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // Assertions.
    let mut status = match &no_convergence {
        Some(msg) => Err(msg.clone()),
        None => Ok("all assertions passed".to_string()),
    };
    for assertion in &sc.asserts {
        let at = now_ms(start);
        let name = format!("{assertion:?}");
        let reconcile = {
            let mut m: Model = BTreeMap::new();
            if let Op::RunJs { program, .. } = &sc.workload.op {
                for step in &program.0 {
                    if let Step::Write { path, bytes } = step {
                        m.insert(path.clone(), bytes.replace("{i}", &fresh_attempted_iteration.unwrap_or_else(|| sc.workload.count.saturating_sub(1)).to_string()).into_bytes());
                    }
                }
            }
            m
        };
        let passed = if *assertion == Assertion::NoUnexpectedJsFailure {
            failed.is_empty()
        } else if *assertion == Assertion::FreshWriteSucceeds {
            fresh_write_acked
        } else {
            check_assertion(assertion, &observer_store, &fs_store, &final_heads, &acked, &unknown, &model, &reconcile).await
        };
        events.push(ScenarioEvent::Assert { at_ms: at, name: name.clone(), passed });
        if !passed {
            status = Err(format!("assertion failed: {name}"));
        }
    }
    store_result(ScenarioResult { status, events, acked, unknown, failed, final_heads, model, artifact_dir: None });
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn check_assertion(
    assertion: &Assertion,
    observer_store: &Arc<LabelStore>,
    fs_store: &Arc<FsStore>,
    final_heads: &BTreeMap<String, Option<String>>,
    acked: &[String],
    _unknown: &[String],
    model: &Model,
    reconcile: &Model,
) -> bool {
    match assertion {
        Assertion::AckedCommitsInLegalHistory => {
            if acked.is_empty() { return true; }
            // Each acknowledged head must appear as a `to` of some reflog
            // entry, or be the final head.
            let tos: Vec<String> = match observer_store.log("main").await {
                Ok(entries) => entries.iter().map(|e| to_hex(&e.to)).collect(),
                Err(_) => return false,
            };
            let final_head = final_heads.values().next().cloned().flatten();
            acked.iter().all(|h| tos.contains(h) || final_head.as_deref() == Some(h.as_str()))
        }
        Assertion::ReplicasConvergeAfterRecovery => {
            let heads: Vec<Option<&String>> = final_heads.values().map(|h| h.as_ref()).collect();
            heads.windows(2).all(|w| w[0] == w[1])
        }
        Assertion::PublishedSnapshotMatchesModel => {
            // The published head's content must match the independent model.
            let final_head = match final_heads.values().next().cloned().flatten() {
                Some(h) => h,
                // Nothing published: the model must also be empty.
                None => return model.is_empty(),
            };
            for (path, expected) in model {
                let actual = read_file_at(fs_store, &final_head, path).await;
                match actual {
                    Some(bytes) if bytes == *expected => {},
                    // A final unknown op that committed is reconciled by
                    // observation: its content matches what it would have
                    // written.
                    Some(bytes) if bytes == *reconcile.get(path).unwrap_or(&bytes) => {},
                    _ => return false,
                }
            }
            true
        }
        Assertion::NoUnexpectedJsFailure => true, // checked against runner failures above
        Assertion::FreshWriteSucceeds => false, // handled by the runner post-recovery probe
    }
}

#[cfg(feature = "real-sled")]
impl Drop for ScenarioHost {
    fn drop(&mut self) {
        self.console_bridge.revoke();
        self._pump.abort();
        self._console_pump.abort();
    }
}
