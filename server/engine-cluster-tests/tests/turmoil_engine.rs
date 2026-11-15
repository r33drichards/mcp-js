//! Real JavaScript execution with file outputs, coordinated through a Raft
//! cluster running inside turmoil: the JS run mounts an fs label, writes a
//! file, and the resulting label head move is CAS'd through the Raft leader.
//! A partitioned follower's push must fail loudly (no false success), and
//! after repair the same push succeeds and converges on every node.
use serde::{Serialize, de::DeserializeOwned};
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
use std::{
    cell::RefCell, collections::HashMap, future::Future, pin::Pin, rc::Rc, sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    task::JoinSet,
};
use turmoil::net::{TcpListener, TcpStream};

const PORT: u16 = 4100;
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
            .join(format!("mcp-turmoil-eng-{tag}-{}", std::process::id()))
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
        "mcp-turmoil-sled-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    sled::Config::new().path(dir).open().expect("sled")
}

/// Poll an engine execution to terminal status; return the resulting fs CA hex.
async fn run_js_to_fs(engine: &Engine, code: &str) -> Option<String> {
    let exec_id = engine
        .run_js(code.to_string())
        .maybe_fs(Some("main".to_string()))
        .execute()
        .await
        .expect("submit");
    for _ in 0..600 {
        tokio::time::sleep(Duration::from_millis(25)).await;
        if let Ok(info) = engine.get_execution(exec_id.clone()) {
            match info.status.as_str() {
                "completed" => return info.fs,
                "failed" => panic!("execution failed: {:?}", info.error),
                "timed_out" => panic!("execution timed out"),
                _ => continue,
            }
        }
    }
    panic!("timeout waiting for execution");
}

#[test]
fn js_file_output_pushes_label_through_raft_under_partition() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        deno_core::v8::V8::set_flags_from_string("--single-threaded");
        server::engine::initialize_v8();
    });

    let shared: Nodes = Rc::new(RefCell::new(HashMap::new()));
    let mut sim = turmoil::Builder::new()
        .rng_seed(21)
        .enable_random_order()
        .min_message_latency(Duration::from_millis(1))
        .max_message_latency(Duration::from_millis(3))
        .simulation_duration(Duration::from_secs(30))
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

        // Engines: the leader's engine runs the JS and mounts "main"; the
        // follower's engine pushes the resulting head through its own
        // (forwarding) cluster node.
        let leader_engine = engine_with_cluster(nodes[leader].clone(), "leader");
        let follower_store =
            Arc::new(LabelStore::in_memory().with_cluster(nodes[follower].clone()));

        // 1. Real JS run on the leader: write a file into the mounted label.
        let code = r#"(async () => {
            await fs.writeFile("/data/report.txt", "generated by js");
        })()"#;
        let ca = run_js_to_fs(&leader_engine, code).await.expect("fs CA id");

        // 2. Partition the pushing follower from the leader. The client code
        //    runs in the observer host, and turmoil attributes sockets to the
        //    host that opens them, so the observer must be partitioned from
        //    the leader too or its forwarded CAS bypasses the fault.
        turmoil::partition(follower, leader);
        turmoil::partition("observer", leader);

        // 3. Moving the label head from the partitioned follower must FAIL
        //    loudly, not hang or report success.
        let ca_id: CaId = parse_ca_hex(&ca).expect("valid ca hex");
        let cas = follower_store.cas("main", None, ca_id, None).await;
        // A partitioned push must be rejected outright, never applied.
        assert!(
            cas.is_err(),
            "partitioned label move must not succeed (got {cas:?})"
        );
        assert!(
            follower_store
                .resolve("main")
                .await
                .ok()
                .flatten()
                .is_none(),
            "partitioned push must not move the label head"
        );

        // 4. Repair; the same head move now succeeds through the Raft leader.
        turmoil::repair(follower, leader);
        turmoil::repair("observer", leader);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let pushed = loop {
            match follower_store.cas("main", None, ca_id, None).await {
                Ok(_) => break Ok::<(), String>(()),
                Err(e) if tokio::time::Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    let _ = e;
                }
                Err(e) => break Err(e),
            }
        };
        pushed.expect("post-repair label move must succeed");

        // 5. The label head is visible on every node's store. The commit is
        //    durable before cas() returns, but followers apply on a later
        //    heartbeat, so reads poll with a bounded deadline.
        let head = loop {
            match follower_store.resolve("main").await {
                Ok(Some(h)) => break h,
                Ok(None) => {}
                Err(e) => panic!("resolve after repair failed: {e}"),
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "pushing follower never applied the committed head"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        };
        for name in ["n0", "n1", "n2"] {
            let s = Arc::new(LabelStore::in_memory().with_cluster(nodes[name].clone()));
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            loop {
                if s.resolve("main").await.ok().flatten() == Some(head) {
                    break;
                }
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "{name}: head never converged"
                );
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
        Ok(())
    });
    sim.run().expect("turmoil simulation");
}

#[test]
fn storage_failure_stops_node_and_cluster_keeps_committing() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        deno_core::v8::V8::set_flags_from_string("--single-threaded");
        server::engine::initialize_v8();
    });

    let shared: Nodes = Rc::new(RefCell::new(HashMap::new()));
    let mut sim = turmoil::Builder::new()
        .rng_seed(21)
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
        let follower_store =
            Arc::new(LabelStore::in_memory().with_cluster(nodes[follower].clone()));

        // Baseline: an ordinary push commits and lands on every node.
        let ca1 = ca_from_hex_hex("11");
        let cas = follower_store.cas("main", None, ca1, None).await;
        assert!(matches!(cas, Ok(true)), "baseline push failed: {cas:?}");
        wait_head_on_all(&nodes, "main", ca1).await;

        // Inject an fs fault on the leader: the next durable write fails and
        // the node must stop itself cleanly instead of serving unknown-state
        // results.
        nodes[leader].sim_storage_failure(true);

        // A push through the failing leader must NOT report success. Whether
        // the fault fires on this very write or the leader is already stopped,
        // the head must stay at ca1.
        let ca2 = ca_from_hex_hex("22");
        let cas = follower_store.cas("main", Some(ca1), ca2, None).await;
        assert!(
            !matches!(cas, Ok(true)),
            "push through a storage-failing leader reported success: {cas:?}"
        );

        // The survivors keep quorum (2 of 3): a new leader emerges and a
        // retry commits. The dead leader's durable head stays at ca1.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        let pushed = loop {
            match follower_store.cas("main", Some(ca1), ca2, None).await {
                Ok(true) => break Ok(()),
                _ if tokio::time::Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                other => break Err(format!("{other:?}")),
            }
        };
        pushed.expect("post-failure push must succeed on the surviving quorum");

        // Every surviving node converges on ca2; the stopped node retains its
        // durable ca1 state and does not fabricate ca2.
        let survivors: Vec<_> = ["n0", "n1", "n2"]
            .into_iter()
            .filter(|n| *n != leader)
            .collect();
        for name in survivors {
            wait_head_on(&nodes[name], "main", ca2).await;
        }
        assert_eq!(
            LabelStore::in_memory()
                .with_cluster(nodes[leader].clone())
                .resolve("main")
                .await
                .ok()
                .flatten(),
            Some(ca1),
            "stopped node must not advance past its durable state"
        );
        Ok(())
    });
    sim.run().expect("turmoil simulation");
}

fn ca_from_hex_hex(byte: &str) -> CaId {
    let mut hex = String::new();
    while hex.len() < 64 {
        hex.push_str(byte);
    }
    server::engine::parse_ca_hex(&hex).expect("valid ca hex")
}

async fn wait_head_on(node: &Arc<ClusterNode>, label: &str, head: CaId) {
    let s = Arc::new(LabelStore::in_memory().with_cluster(node.clone()));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if s.resolve(label).await.ok().flatten() == Some(head) {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{label}: head never converged"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn wait_head_on_all(nodes: &HashMap<String, Arc<ClusterNode>>, label: &str, head: CaId) {
    for name in ["n0", "n1", "n2"] {
        wait_head_on(&nodes[name], label, head).await;
    }
}
