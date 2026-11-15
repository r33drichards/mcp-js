//! Fuzz target combining generated JavaScript execution with the cluster
//! fault model: input bytes seed the turmoil scheduler, generate JS payloads,
//! and select partition/repair actions around label pushes. Committed heads
//! must never regress, a partitioned push must never succeed, and after
//! repairing all links every node must converge on the last committed head.
#![no_main]

use libfuzzer_sys::fuzz_target;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use server::cluster::{
    AppendEntriesRequest, AppendEntriesResponse, CasRequest, CasResponse, ClusterConfig,
    ClusterNode, ForwardAck, JoinRequest, LeaveRequest, PeerRpc, PeerRpcFuture, PutRequest,
    RequestVoteRequest, RequestVoteResponse, Role,
};
use server::engine::Engine;
use server::engine::execution::ExecutionRegistry;
use server::engine::fs::FsConfig;
use server::engine::fs_labels::CaId;
use server::engine::fs_labels::LabelStore;
use server::engine::fs_store::FsStore;
use server::engine::opa::{EvalMode, PolicyChain};
use server::engine::parse_ca_hex;
use std::{cell::RefCell, collections::HashMap, rc::Rc, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    task::JoinSet,
};
use turmoil::net::{TcpListener, TcpStream};

const PORT: u16 = 4200;
type RpcFuture<'a, T> = PeerRpcFuture<'a, T>;

struct SimRpc;
impl PeerRpc for SimRpc {
    fn request_vote<'a>(
        &'a self,
        peer: &'a str,
        req: RequestVoteRequest,
    ) -> RpcFuture<'a, RequestVoteResponse> {
        Box::pin(async move { exchange(peer, "/raft/request-vote", &req).await })
    }
    fn append_entries<'a>(
        &'a self,
        peer: &'a str,
        req: AppendEntriesRequest,
    ) -> RpcFuture<'a, AppendEntriesResponse> {
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
    tokio::time::timeout(Duration::from_millis(150), async {
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

fn engine_with_cluster(node: Arc<ClusterNode>, tag: &str) -> Arc<Engine> {
    let registry = ExecutionRegistry::new(
        std::env::temp_dir()
            .join(format!("mcp-fuzz-js-eng-{tag}-{}", std::process::id()))
            .to_str()
            .unwrap(),
    )
    .expect("registry");
    let fs_config = FsConfig::new(Arc::new(PolicyChain::new(vec![], EvalMode::All)));
    let store = Arc::new(FsStore::in_memory());
    let labels = LabelStore::in_memory().with_cluster(node);
    Engine::from_engine(
        Engine::new_stateless(64 * 1024 * 1024, 30, 4)
            .with_fs_config(fs_config)
            .with_execution_registry(Arc::new(registry))
            .with_fs_snapshots(store, Arc::new(labels)),
    )
}

type Nodes = Rc<RefCell<HashMap<String, Arc<ClusterNode>>>>;

fn temp_sled(tag: &str) -> sled::Db {
    let dir = std::env::temp_dir().join(format!(
        "mcp-fuzz-js-sled-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    sled::Config::new().path(dir).open().expect("sled")
}

async fn run_js_to_fs(engine: &Engine, code: &str) -> Option<String> {
    let exec_id = engine
        .run_js(code.to_string())
        .maybe_fs(Some("main".to_string()))
        .execute()
        .await
        .ok()?;
    for _ in 0..400 {
        tokio::time::sleep(Duration::from_millis(25)).await;
        if let Ok(info) = engine.get_execution(exec_id.clone()) {
            match info.status.as_str() {
                "completed" => return info.fs,
                "failed" | "timed_out" => return None,
                _ => continue,
            }
        }
    }
    None
}

/// Turn bytes into a printable ASCII payload safe to embed in a JS string.
fn safe_payload(bytes: &[u8]) -> String {
    bytes
        .iter()
        .filter(|b| b.is_ascii_alphanumeric() || **b == b'-' || **b == b'_' || **b == b'.')
        .map(|b| *b as char)
        .collect()
}

fn js_write_file(n: usize, payload: &str) -> String {
    format!("(async () => {{\n  await fs.writeFile(\"/data/fuzz-{n}.txt\", \"{payload}\");\n}})()")
}

/// A partitioned push must fail and must not move the head; a repaired push
/// commits and every node converges on it.
async fn push_and_check(
    store: &LabelStore,
    expect_success: bool,
    ca_id: CaId,
    label: &str,
) -> bool {
    // Repeat pushes must build on the current head; a blind expect=None
    // would falsely conflict with the label this target created earlier.
    let expect = store.resolve(label).await.ok().flatten();
    let cas = store.cas(label, expect, ca_id, None).await;
    let applied = matches!(&cas, Ok(true));
    if expect_success {
        assert!(applied, "{label}: push must succeed (got {cas:?})");
    } else {
        assert!(
            cas.is_err(),
            "{label}: partitioned push must fail (got {cas:?})"
        );
        assert!(
            store.resolve(label).await.ok().flatten().is_none(),
            "{label}: partitioned push must not move the head"
        );
    }
    applied
}

/// After a storage failure the cluster re-elects from the survivors; pushes
/// retry until the new leader serves them. Expect always tracks the current
/// head so a retry never conflicts with an earlier successful push.
async fn push_with_retry(store: &LabelStore, label: &str, ca_id: CaId) -> bool {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let expect = store.resolve(label).await.ok().flatten();
        match store.cas(label, expect, ca_id, None).await {
            Ok(true) => return true,
            Ok(false) => return false,
            Err(_) if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(_) => return false,
        }
    }
}

async fn converge_all(
    nodes: &HashMap<String, Arc<ClusterNode>>,
    label: &str,
    head: CaId,
    exclude: &[String],
) {
    for name in ["n0", "n1", "n2"] {
        if exclude.contains(&name.to_string()) {
            // A node stopped by an injected storage failure keeps its durable
            // state; it is checked separately, not converged.
            continue;
        }
        let s = Arc::new(LabelStore::in_memory().with_cluster(nodes[name].clone()));
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            if s.resolve(label).await.ok().flatten() == Some(head) {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "{label}: head never converged on {name}"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

fuzz_target!(|data: &[u8]| {
    if data.len() < 9 {
        return;
    }
    let seed = u64::from_le_bytes(data[..8].try_into().unwrap());
    let actions: Vec<u8> = data[8..].iter().copied().take(12).collect();
    let data_owned: Vec<u8> = data.to_vec();

    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        deno_core::v8::V8::set_flags_from_string("--single-threaded");
        server::engine::initialize_v8();
    });

    let shared: Nodes = Rc::new(RefCell::new(HashMap::new()));
    let mut sim = turmoil::Builder::new()
        .rng_seed(seed)
        .enable_random_order()
        .min_message_latency(Duration::from_millis(1))
        .max_message_latency(Duration::from_millis(3))
        .simulation_duration(Duration::from_secs(60))
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
        sim.host(name, move || {
            let shared = shared.clone();
            let config = config.clone();
            async move {
                let node =
                    ClusterNode::new(config, temp_sled(name)).with_peer_rpc(Arc::new(SimRpc));
                let listener = TcpListener::bind(("0.0.0.0", PORT)).await?;
                shared.borrow_mut().insert(name.to_owned(), node.clone());
                node.start_background_tasks().expect("start loops");
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
                            if let Some(Err(panic)) = done { panic!("handler panicked: {panic}"); }
                        }
                    }
                }
            }
        });
    }
    sim.client("observer", async move {
        tokio::time::sleep(Duration::from_millis(600)).await;
        let nodes = shared.borrow().clone();
        let mut statuses = Vec::new();
        for name in ["n0", "n1", "n2"] {
            statuses.push((name, nodes[name].status().await));
        }
        let leader = statuses
            .iter()
            .find(|(_, s)| s.role == Role::Leader)
            .map(|(n, _)| *n)
            .expect("leader elected");
        let follower = statuses
            .iter()
            .find(|(_, s)| s.role == Role::Follower)
            .map(|(n, _)| *n)
            .expect("follower exists");

        let leader_engine = engine_with_cluster(nodes[leader].clone(), "leader");
        let follower_store =
            Arc::new(LabelStore::in_memory().with_cluster(nodes[follower].clone()));

        // The observer host hosts the client code: turmoil attributes sockets
        // to the opening host, so partitioning the observer from the leader is
        // part of every partition action.
        let mut partitioned = std::collections::HashSet::new();
        let mut committed: Option<CaId> = None;
        // Nodes stopped by an injected storage failure, with the head they
        // durably hold (frozen at failure time).
        let mut failed: Vec<(String, Option<CaId>)> = Vec::new();
        let mut runs = 0usize;

        for (i, action) in actions.iter().enumerate() {
            let (opcode, pick) = (action >> 4, action & 0x0f);
            match opcode {
                // Run generated JS and push the resulting head from the follower.
                0 => {
                    // The tail can be shorter than the action-indexed payload
                    // offsets imply, so clamp both ends before slicing.
                    let start = (8 + i * 3).min(data_owned.len());
                    let payload =
                        safe_payload(&data_owned[start..(start + 3).min(data_owned.len())]);
                    let code = js_write_file(runs, &payload);
                    runs += 1;
                    let Some(ca_hex) = run_js_to_fs(&leader_engine, &code).await else {
                        continue;
                    };
                    let ca_id: CaId = parse_ca_hex(&ca_hex).expect("valid ca hex");
                    let applied = if failed.is_empty() {
                        push_and_check(
                            &follower_store,
                            !partitioned.contains("leader"),
                            ca_id,
                            "push",
                        )
                        .await
                    } else {
                        push_with_retry(&follower_store, "push", ca_id).await
                    };
                    if applied {
                        committed = Some(ca_id);
                        for name in ["n0", "n1", "n2"] {
                            let st = nodes[name].status().await;
                            eprintln!(
                                "PUSHCHK {name}: role={:?} term={} commit={} applied={} loglen={}",
                                st.role, st.term, st.commit_index, st.last_applied, st.log_length
                            );
                        }
                    }
                }
                // Partition a pair chosen from the input.
                1 => {
                    let (a, b) = pair(pick, leader, follower);
                    turmoil::partition(a, b);
                    partitioned.insert(a.to_string());
                    if b == leader {
                        partitioned.insert("leader".to_string());
                    }
                }
                // Repair a pair chosen from the input.
                2 => {
                    let (a, b) = pair(pick, leader, follower);
                    turmoil::repair(a, b);
                    if b == leader {
                        partitioned.clear();
                    }
                }
                // Invariant: the committed head is visible on every node.
                3 => {
                    if let Some(head) = committed {
                        let names: Vec<String> = failed.iter().map(|(n, _)| n.clone()).collect();
                        converge_all(&nodes, "push", head, &names).await;
                    }
                }
                // Inject an fs fault: the leader's next durable write fails
                // and the node stops itself. The committed head must stay
                // durable and the survivors must keep committing.
                4 => {
                    if failed.iter().all(|(n, _)| n != leader) {
                        failed.push((leader.to_string(), committed.clone()));
                        nodes[leader].sim_storage_failure(true);
                    }
                }
                // Run raw arbitrary JS; failures are tolerated, only panics
                // inside the engine count as crashes.
                _ => {
                    let start = (8 + i * 2).min(data_owned.len());
                    let code = String::from_utf8_lossy(
                        &data_owned[start..(start + 24).min(data_owned.len())],
                    );
                    let _ = run_js_to_fs(&leader_engine, &code).await;
                }
            }
        }

        // Final repair and convergence on the last committed head.
        for a in ["n0", "n1", "n2"] {
            for b in ["n0", "n1", "n2"] {
                if a != b {
                    turmoil::repair(a, b);
                }
            }
        }
        partitioned.clear();
        if let Some(head) = committed {
            let names: Vec<String> = failed.iter().map(|(n, _)| n.clone()).collect();
            converge_all(&nodes, "push", head, &names).await;
        } else {
            // No successful push happened; a final push must succeed.
            let payload = safe_payload(&data_owned[data_owned.len().saturating_sub(4)..]);
            let code = js_write_file(runs + 1000, &payload);
            if let Some(ca_hex) = run_js_to_fs(&leader_engine, &code).await {
                let ca_id: CaId = parse_ca_hex(&ca_hex).expect("valid ca hex");
                let applied = if failed.is_empty() {
                    push_and_check(&follower_store, true, ca_id, "final").await
                } else {
                    push_with_retry(&follower_store, "final", ca_id).await
                };
                assert!(
                    applied || !failed.is_empty(),
                    "final push must succeed when no storage failure occurred"
                );
                if applied {
                    let names: Vec<String> = failed.iter().map(|(n, _)| n.clone()).collect();
                    converge_all(&nodes, "final", ca_id, &names).await;
                }
            }
        }
        // A node stopped by an injected storage failure must hold exactly the
        // head it had at failure time: durable, frozen, never fabricated.
        for (name, expected) in &failed {
            let got = LabelStore::in_memory()
                .with_cluster(nodes[name].clone())
                .resolve("push")
                .await
                .ok()
                .flatten();
            assert_eq!(
                got, *expected,
                "{name}: durable head changed after storage failure"
            );
        }
        Ok(())
    });
    sim.run().expect("turmoil simulation");
});

fn pair<'a>(pick: u8, leader: &'a str, follower: &'a str) -> (&'a str, &'a str) {
    match pick % 6 {
        0 => ("observer", leader),
        1 => (follower, leader),
        2 => (leader, follower),
        3 => ("observer", follower),
        4 => (follower, "n0"),
        _ => (leader, follower),
    }
}
