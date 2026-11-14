//! Real three-node elections over turmoil TCP, not reqwest's OS sockets.
use scheduler_raft_audit::cluster::{
    AppendEntriesRequest, AppendEntriesResponse, ClusterConfig, ClusterNode, PeerRpc,
    RequestVoteRequest, RequestVoteResponse, Role, serve_cluster_connection,
};
use serde::{Serialize, de::DeserializeOwned};
use std::{
    cell::RefCell, collections::HashMap, future::Future, pin::Pin, rc::Rc, sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    task::JoinSet,
};
use turmoil::net::{TcpListener, TcpStream};

const PORT: u16 = 4000;
type RpcFuture<'a, T> = Pin<Box<dyn Future<Output = Option<T>> + Send + 'a>>;

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
            "POST {path} HTTP/1.1\r\nHost: {name}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()
        );
        stream.write_all(head.as_bytes()).await.ok()?;
        stream.write_all(&body).await.ok()?;
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).await.ok()?;
        let raw = String::from_utf8(raw).ok()?;
        let (head, body) = raw.split_once("\r\n\r\n")?;
        if !head.starts_with("HTTP/1.1 200") { return None; }
        if let Some(line) = head.lines().find(|line| line.to_ascii_lowercase().starts_with("content-length:")) {
            let length: usize = line.split_once(':')?.1.trim().parse().ok()?;
            if body.len() != length { return None; }
        }
        serde_json::from_str(body).ok()
    }).await.ok().flatten()
}

pub type Nodes = Rc<RefCell<HashMap<String, Arc<ClusterNode>>>>;

pub fn run_cluster<F, Fut>(seed: u64, test: F)
where
    F: FnOnce(Nodes) -> Fut,
    Fut: Future<Output = turmoil::Result> + 'static,
{
    let shared = Rc::new(RefCell::new(HashMap::<String, Arc<ClusterNode>>::new()));
    let mut sim = turmoil::Builder::new()
        .rng_seed(seed)
        .enable_random_order()
        .min_message_latency(Duration::from_millis(1))
        .max_message_latency(Duration::from_millis(3))
        .simulation_duration(Duration::from_secs(10))
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
                let node = ClusterNode::new(config, sled::Config::new().temporary(true).open()?)
                    .with_peer_rpc(Arc::new(SimRpc));
                let listener = TcpListener::bind(("0.0.0.0", PORT)).await?;
                shared.borrow_mut().insert(node.status().await.node_id.clone(), node.clone());
                node.start_background_tasks().expect("start election and heartbeat loops");
                let mut connections = JoinSet::new();
                loop {
                    tokio::select! {
                        incoming = listener.accept() => {
                            let (stream, _) = incoming?;
                            let node = node.clone();
                            connections.spawn(async move { serve_cluster_connection(node, stream).await });
                        }
                        done = connections.join_next(), if !connections.is_empty() => {
                            if let Some(Err(panic)) = done { panic!("cluster handler panicked: {panic}"); }
                        }
                    }
                }
            }
        });
    }
    sim.client("observer", test(shared));
    sim.run()
        .unwrap_or_else(|e| panic!("peer simulation seed={seed}: {e}"));
}

/// LibFuzzer/CI entry point: first eight bytes seed the scheduler, the rest
/// select up to twelve network faults or client proposals.
pub fn run_fault_case(input: &[u8]) {
    if input.len() < 8 {
        return;
    }
    let seed = u64::from_le_bytes(input[..8].try_into().unwrap());
    let actions: Vec<u8> = input[8..].iter().copied().take(12).collect();
    run_cluster(seed, move |shared| async move {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let nodes = shared.borrow().clone();
        assert_eq!(nodes.len(), 3, "seed={seed}: missing simulation host");
        let old = nodes["n0"].clone();
        assert_eq!(
            old.status().await.role,
            Role::Leader,
            "seed={seed}: initial election"
        );
        tokio::time::timeout(
            Duration::from_secs(1),
            old.put("fuzz/committed-prefix".into(), "stable".into()),
        )
        .await
        .expect("baseline timed out")
        .expect("baseline majority commit");
        for _ in 0..40 {
            let mut applied = 0;
            for node in nodes.values() {
                if node
                    .get("fuzz/committed-prefix")
                    .await
                    .expect("read prefix")
                    .as_deref()
                    == Some("stable")
                {
                    applied += 1;
                }
            }
            if applied == 3 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        for node in nodes.values() {
            assert_eq!(
                node.get("fuzz/committed-prefix")
                    .await
                    .expect("read prefix")
                    .as_deref(),
                Some("stable"),
                "seed={seed}: baseline must reach every node before faults"
            );
        }
        let mut terms = [0u64; 3];
        for (step, action) in actions.iter().enumerate() {
            match action % 9 {
                0 => turmoil::partition("n0", "n1"),
                1 => turmoil::partition("n0", "n2"),
                2 => turmoil::repair("n0", "n1"),
                3 => turmoil::repair("n0", "n2"),
                4 => turmoil::partition_oneway("n1", "n0"),
                5 => turmoil::partition_oneway("n0", "n2"),
                6 | 7 => {
                    let target = ["n0", "n1", "n2"][((action >> 4) % 3) as usize];
                    let _ = tokio::time::timeout(
                        Duration::from_millis(120),
                        nodes[target]
                            .put(format!("fuzz/proposal-{step}"), format!("value-{action}")),
                    )
                    .await;
                }
                _ => tokio::time::sleep(Duration::from_millis(15 + (action >> 4) as u64)).await,
            }
            let mut statuses = Vec::new();
            for (i, name) in ["n0", "n1", "n2"].iter().enumerate() {
                let status = nodes[*name].status().await;
                assert!(
                    status.term >= terms[i],
                    "seed={seed} step={step} {name}: term regressed"
                );
                terms[i] = status.term;
                assert!(
                    status.commit_index <= status.log_length,
                    "seed={seed} step={step} {name}: committed beyond log: {status:?}"
                );
                assert!(
                    status.last_applied <= status.commit_index,
                    "seed={seed} step={step} {name}: applied beyond commit: {status:?}"
                );
                assert_eq!(
                    nodes[*name]
                        .get("fuzz/committed-prefix")
                        .await
                        .expect("read prefix")
                        .as_deref(),
                    Some("stable"),
                    "seed={seed} step={step} {name}: committed prefix changed"
                );
                statuses.push(status);
            }
            for a in 0..3 {
                for b in a + 1..3 {
                    assert!(
                        !(statuses[a].role == Role::Leader
                            && statuses[b].role == Role::Leader
                            && statuses[a].term == statuses[b].term),
                        "seed={seed} step={step}: two leaders in term {}",
                        statuses[a].term
                    );
                }
            }
        }
        for peer in ["n1", "n2"] {
            turmoil::repair("n0", peer);
        }
        turmoil::repair("n1", "n2");
        let mut committed = false;
        for _ in 0..60 {
            let mut leaders = Vec::new();
            for name in ["n0", "n1", "n2"] {
                if nodes[name].status().await.role == Role::Leader {
                    leaders.push(name);
                }
            }
            if leaders.len() == 1 {
                if let Ok(Ok(())) = tokio::time::timeout(
                    Duration::from_millis(250),
                    nodes[leaders[0]].put("fuzz/after-repair".into(), "recovered".into()),
                )
                .await
                {
                    committed = true;
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(committed, "seed={seed}: repaired majority did not commit");
        let mut converged = false;
        for _ in 0..60 {
            let mut applied = 0;
            for node in nodes.values() {
                if node
                    .get("fuzz/after-repair")
                    .await
                    .expect("read recovered")
                    .as_deref()
                    == Some("recovered")
                {
                    applied += 1;
                }
            }
            if applied == 3 {
                converged = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(converged, "seed={seed}: repaired node did not catch up");
        Ok(())
    });
}
