//! Three real sled-backed Raft nodes over Turmoil TCP. The observer never
//! touches a node handle: all data traffic runs in the owning host over RPC.
#![cfg(feature = "cluster")]

use scheduler_raft_audit::cluster::{
    serve_cluster_connection, AppendEntriesRequest, AppendEntriesResponse, CasRequest, CasResponse,
    ClusterConfig, ClusterNode, ForwardAck, GetResponse, JoinRequest, LeaveRequest, PeerRpc,
    PutRequest, RequestVoteRequest, RequestVoteResponse,
};
use serde::{de::DeserializeOwned, Serialize};
use std::{
    future::Future,
    mem::ManuallyDrop,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    task::JoinSet,
};
use turmoil::{
    fs::shim::std::fs,
    net::{TcpListener, TcpStream},
};

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

async fn http<R: DeserializeOwned>(peer: &str, method: &str, path: &str, body: &[u8]) -> Option<R> {
    tokio::time::timeout(Duration::from_millis(350), async {
        let (name, port) = peer.rsplit_once(':')?;
        let mut stream = TcpStream::connect((name, port.parse::<u16>().ok()?)).await.ok()?;
        let head = format!("{method} {path} HTTP/1.1\r\nHost: {name}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
        stream.write_all(head.as_bytes()).await.ok()?;
        stream.write_all(body).await.ok()?;
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).await.ok()?;
        let raw = String::from_utf8(raw).ok()?;
        let (head, body) = raw.split_once("\r\n\r\n")?;
        if !head.starts_with("HTTP/1.1 200") { return None; }
        let len: usize = head.lines().find(|l| l.to_ascii_lowercase().starts_with("content-length:"))?
            .split_once(':')?.1.trim().parse().ok()?;
        if body.len() != len { return None; }
        serde_json::from_str(body).ok()
    }).await.ok().flatten()
}

async fn exchange<Q: Serialize, R: DeserializeOwned>(peer: &str, path: &str, req: &Q) -> Option<R> {
    http(peer, "POST", path, &serde_json::to_vec(req).ok()?).await
}
async fn get(peer: &str) -> Option<String> {
    let response: GetResponse = http(peer, "GET", "/data/get/receipt", &[]).await?;
    response.value
}
async fn put(peer: &str, value: &str) -> bool {
    exchange::<_, ForwardAck>(
        peer,
        "/data/put",
        &PutRequest {
            key: "receipt".into(),
            value: value.into(),
            extra: vec![],
        },
    )
    .await
    .is_some()
}

fn config(name: &str, i: usize) -> ClusterConfig {
    let peers = ["n0", "n1", "n2"]
        .into_iter()
        .filter(|p| *p != name)
        .map(|p| format!("{p}:{PORT}"))
        .collect();
    let peer_addrs = ["n0", "n1", "n2"]
        .into_iter()
        .filter(|p| *p != name)
        .map(|p| (p.to_owned(), format!("{p}:{PORT}")))
        .collect();
    let timeout = [70, 180, 300][i];
    ClusterConfig {
        node_id: name.to_owned(),
        peers,
        peer_addrs,
        cluster_port: PORT,
        advertise_addr: Some(format!("{name}:{PORT}")),
        heartbeat_interval: Duration::from_millis(20),
        election_timeout_min: Duration::from_millis(timeout),
        election_timeout_max: Duration::from_millis(timeout),
        learner: false,
    }
}

fn run(seed: u64) -> turmoil::Result<Vec<String>> {
    let stage = Arc::new(AtomicUsize::new(0));
    let phase = Arc::new(AtomicUsize::new(0));
    let fill = Arc::new(AtomicBool::new(false));
    let filled = Arc::new(AtomicBool::new(false));
    let reopened = Arc::new(AtomicBool::new(false));
    let reopened_again = Arc::new(AtomicBool::new(false));
    let history = Arc::new(Mutex::new(Vec::<String>::new()));
    let mut builder = turmoil::Builder::new();
    builder
        .rng_seed(seed)
        .enable_random_order()
        .min_message_latency(Duration::from_millis(1))
        .max_message_latency(Duration::from_millis(3))
        .simulation_duration(Duration::from_secs(30));
    builder.fs().capacity(32_768).sync_probability(0.0);
    let mut sim = builder.build();

    for (i, name) in ["n0", "n1", "n2"].into_iter().enumerate() {
        let cfg = config(name, i);
        let phase = phase.clone();
        let fill = fill.clone();
        let filled = filled.clone();
        let reopened = reopened.clone();
        let reopened_again = reopened_again.clone();
        let history = history.clone();
        sim.host(name, move || {
            let cfg = cfg.clone();
            let phase = phase.clone();
            let fill = fill.clone();
            let filled = filled.clone();
            let reopened = reopened.clone();
            let reopened_again = reopened_again.clone();
            let history = history.clone();
            async move {
                let db = sled::Config::new().path("/raft").flush_every_ms(None)
                    .segment_size(4096).cache_capacity(1024 * 1024).open()?;
                let node = ClusterNode::new(cfg, db).with_peer_rpc(Arc::new(SimRpc));
                if name == "n0" && phase.load(Ordering::SeqCst) > 0 {
                    let second = phase.load(Ordering::SeqCst) == 2;
                    let expected = if second { "value-2" } else { "value-1" };
                    assert_eq!(node.get("receipt").await?, Some(expected.into()), "seed={seed} reopened n0");
                    history.lock().unwrap().push(format!("reopened:{expected}"));
                    if second { reopened_again.store(true, Ordering::SeqCst); }
                    else { reopened.store(true, Ordering::SeqCst); }
                }
                // Crash and simulator teardown may drop host futures outside
                // their fs context. Keep one Arc per host incarnation so sled Drop
                // cannot flush there; never reuse these bounded-run handles.
                let _no_destructor_flush = ManuallyDrop::new(node.clone());
                let listener = TcpListener::bind(("0.0.0.0", PORT)).await?;
                node.start_background_tasks().expect("start Raft tasks");
                let mut connections = JoinSet::new();
                let mut faulted = false;
                loop {
                    tokio::select! {
                        incoming = listener.accept() => {
                            let (stream, _) = incoming?;
                            let node = node.clone();
                            connections.spawn(async move { serve_cluster_connection(node, stream).await });
                        }
                        done = connections.join_next(), if !connections.is_empty() => {
                            if let Some(Err(panic)) = done { panic!("handler panic: {panic}"); }
                        }
                        _ = tokio::time::sleep(Duration::from_millis(5)), if name == "n0" && !faulted => {
                            if !fill.load(Ordering::SeqCst) { continue; }
                            use std::io::Write;
                            let mut filler = fs::File::create("/filler")?;
                            let mut exhausted = false;
                            for _ in 0..256 {
                                match filler.write(&[0; 256]) {
                                    Ok(0) => panic!("filler write made no progress"),
                                    Ok(_) => {},
                                    Err(e) => { assert!(e.to_string().contains("No space left"), "{e}"); exhausted = true; break; }
                                }
                            }
                            assert!(exhausted, "filler did not exhaust the 32-KiB disk");
                            // The filler is pending, so Sim::crash removes it.
                            std::mem::forget(filler);
                            faulted = true;
                            filled.store(true, Ordering::SeqCst);
                        }
                    }
                }
            }
        });
    }
    let sc = stage.clone();
    let ph = phase.clone();
    let fi = fill.clone();
    let fd = filled.clone();
    let re = reopened.clone();
    let re2 = reopened_again.clone();
    let h = history.clone();
    sim.client("observer", async move {
        let mut acked_1 = false;
        for _ in 0..200 {
            if put("n1:4000", "value-1").await {
                acked_1 = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(acked_1, "seed={seed} value-1 lacked an HTTP acknowledgment");
        for name in ["n0", "n1", "n2"] {
            for _ in 0..200 {
                if get(&format!("{name}:{PORT}")).await.as_deref() == Some("value-1") {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert_eq!(
                get(&format!("{name}:{PORT}")).await.as_deref(),
                Some("value-1"),
                "seed={seed} host={name} before fault"
            );
        }
        h.lock().unwrap().push("acked:value-1".into());
        fi.store(true, Ordering::SeqCst);
        for _ in 0..200 {
            if fd.load(Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(fd.load(Ordering::SeqCst), "disk-full injection not reached");
        let mut acked_2 = false;
        for _ in 0..300 {
            if put("n1:4000", "value-2").await {
                acked_2 = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(acked_2, "seed={seed} value-2 lacked an HTTP acknowledgment");
        for name in ["n1", "n2"] {
            for _ in 0..200 {
                if get(&format!("{name}:{PORT}")).await.as_deref() == Some("value-2") {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert_eq!(
                get(&format!("{name}:{PORT}")).await.as_deref(),
                Some("value-2"),
                "seed={seed} host={name} after fault"
            );
        }
        h.lock().unwrap().push("quorum:value-2".into());
        sc.store(1, Ordering::SeqCst);
        while ph.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        for _ in 0..200 {
            if re.load(Ordering::SeqCst) && get("n0:4000").await.as_deref() == Some("value-2") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(re.load(Ordering::SeqCst), "restarted n0 did not open");
        assert_eq!(
            get("n0:4000").await.as_deref(),
            Some("value-2"),
            "seed={seed} n0 did not converge"
        );
        h.lock().unwrap().push("converged:value-2".into());
        sc.store(2, Ordering::SeqCst);
        while ph.load(Ordering::SeqCst) < 2 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        for _ in 0..200 {
            if re2.load(Ordering::SeqCst) && get("n0:4000").await.as_deref() == Some("value-2") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            re2.load(Ordering::SeqCst),
            "second n0 reopen did not complete"
        );
        assert_eq!(
            get("n0:4000").await.as_deref(),
            Some("value-2"),
            "seed={seed} second n0 recovery lost value-2"
        );
        sc.store(3, Ordering::SeqCst);
        Ok(())
    });
    for _ in 0..50_000 {
        sim.step()?;
        if stage.load(Ordering::SeqCst) == 1 {
            break;
        }
    }
    assert_eq!(
        stage.load(Ordering::SeqCst),
        1,
        "seed={seed} first phase did not finish"
    );
    sim.crash("n0");
    phase.store(1, Ordering::SeqCst);
    sim.bounce("n0");
    for _ in 0..50_000 {
        sim.step()?;
        if stage.load(Ordering::SeqCst) == 2 {
            break;
        }
    }
    assert_eq!(
        stage.load(Ordering::SeqCst),
        2,
        "seed={seed} recovery phase did not finish"
    );
    sim.crash("n0");
    phase.store(2, Ordering::SeqCst);
    sim.bounce("n0");
    for _ in 0..50_000 {
        sim.step()?;
        if stage.load(Ordering::SeqCst) == 3 {
            break;
        }
    }
    assert_eq!(
        stage.load(Ordering::SeqCst),
        3,
        "seed={seed} second recovery did not finish"
    );
    let result = history.lock().unwrap().clone();
    Ok(result)
}

#[test]
fn real_three_node_quorum_survives_disk_full_and_crash_reopen() -> turmoil::Result {
    for seed in [7, 42, 314] {
        let first = run(seed)?;
        assert_eq!(
            first,
            [
                "acked:value-1",
                "quorum:value-2",
                "reopened:value-1",
                "converged:value-2",
                "reopened:value-2"
            ]
        );
        assert_eq!(run(seed)?, first, "seed={seed} replay changed");
    }
    Ok(())
}
