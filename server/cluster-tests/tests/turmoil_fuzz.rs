//! Bounded, replayable fault-sequence fuzzing of the real cluster HTTP handler.
use scheduler_raft_audit::cluster::{
    AppendEntriesRequest, AppendEntriesResponse, ClusterConfig, ClusterNode, ClusterStatus,
    LogEntry, RequestVoteRequest, RequestVoteResponse, serve_cluster_connection,
};
use std::{cell::RefCell, error::Error, rc::Rc, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    task::JoinSet,
};
use turmoil::net::{TcpListener, TcpStream};

const PORT: u16 = 4000;
const RECEIPT: &str = "fuzz/committed-receipt";
const STABLE_VALUE: &str = "committed-once";
const STEPS: usize = 24;

// No ambient randomness: the workload and turmoil's scheduler both derive from
// the same printed seed, so a failed case can be replayed verbatim.
fn draw(state: &mut u64) -> u64 {
    *state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
    *state ^ (*state >> 33)
}

// A failed connection, timeout, or incomplete reply is expected under faults;
// an intact HTTP response is checked by the caller.
async fn request(method: &str, path: &str, body: &[u8]) -> Option<(u16, String)> {
    let exchange = async {
        let mut stream = TcpStream::connect(("node", PORT)).await?;
        let head = format!(
            "{method} {path} HTTP/1.1\r\nHost: node\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        stream.write_all(head.as_bytes()).await?;
        stream.write_all(body).await?;
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).await?;
        let reply = String::from_utf8(raw)?;
        let (head, body) = reply
            .split_once("\r\n\r\n")
            .ok_or("incomplete HTTP headers")?;
        if let Some(line) = head
            .lines()
            .find(|line| line.to_ascii_lowercase().starts_with("content-length:"))
        {
            let expected: usize = line
                .split_once(':')
                .ok_or("invalid content length")?
                .1
                .trim()
                .parse()?;
            if body.len() != expected {
                return Err::<(u16, String), Box<dyn Error>>("incomplete HTTP body".into());
            }
        }
        let code: u16 = head
            .split_whitespace()
            .nth(1)
            .ok_or("missing status")?
            .parse()?;
        Ok::<_, Box<dyn Error>>((code, body.to_owned()))
    };
    match tokio::time::timeout(Duration::from_millis(150), exchange).await {
        Ok(Ok(response)) => Some(response),
        _ => None,
    }
}

async fn check_safety(node: &ClusterNode, last_term: &mut u64, seed: u64, step: usize) {
    let status = node.status().await;
    assert!(
        status.term >= *last_term,
        "seed={seed} step={step}: term regressed: {status:?}"
    );
    *last_term = status.term;
    assert!(
        status.commit_index >= 1,
        "seed={seed} step={step}: lost commit: {status:?}"
    );
    assert!(
        status.commit_index <= status.log_length,
        "seed={seed} step={step}: committed past log: {status:?}"
    );
    assert!(
        status.last_applied <= status.commit_index,
        "seed={seed} step={step}: applied past commit: {status:?}"
    );
    assert_eq!(
        node.get(RECEIPT)
            .await
            .expect("read committed value")
            .as_deref(),
        Some(STABLE_VALUE),
        "seed={seed} step={step}: committed value changed"
    );
}

fn run_seed(seed: u64) {
    let shared = Rc::new(RefCell::new(None::<Arc<ClusterNode>>));
    let server_shared = shared.clone();
    let mut sim = turmoil::Builder::new()
        .rng_seed(seed)
        .enable_random_order()
        .min_message_latency(Duration::from_millis(1))
        .max_message_latency(Duration::from_millis(4))
        .fail_rate(0.03)
        .repair_rate(0.35)
        .simulation_duration(Duration::from_secs(20))
        .build();

    sim.host("node", move || {
        let shared = server_shared.clone();
        async move {
            let node = ClusterNode::new(
                ClusterConfig { node_id: "node".into(), ..ClusterConfig::default() },
                sled::Config::new().temporary(true).open()?,
            );
            // Establish a committed prefix that arbitrary later RPCs may not rewrite.
            let boot = AppendEntriesRequest {
                term: 1, leader_id: "bootstrap".into(), prev_log_index: 0, prev_log_term: 0,
                entries: vec![LogEntry {
                    term: 1, index: 1, key: RECEIPT.into(), value: STABLE_VALUE.into(), extra: vec![],
                }],
                leader_commit: 1, peer_addrs: None, learner_addrs: None,
            };
            assert!(node.handle_append_entries(boot).await.success);
            let listener = TcpListener::bind(("0.0.0.0", PORT)).await?;
            *shared.borrow_mut() = Some(node.clone());
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    incoming = listener.accept() => {
                        let (stream, _) = incoming?;
                        let node = node.clone();
                        connections.spawn(async move { serve_cluster_connection(node, stream).await });
                    }
                    done = connections.join_next(), if !connections.is_empty() => {
                        if let Some(Err(panic)) = done {
                            panic!("cluster handler panicked: {panic}");
                        }
                        // Hyper may return an I/O error when turmoil drops a connection.
                    }
                }
            }
        }
    });

    sim.client("client", async move {
        let node = loop {
            if let Some(node) = shared.borrow().clone() {
                break node;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        };
        let mut last_term = 0;
        check_safety(&node, &mut last_term, seed, 0).await;
        // Prove that the simulation actually reached the production HTTP handler.
        let mut reached = false;
        for _ in 0..20 {
            if let Some((code, body)) = request("GET", "/raft/status", b"").await {
                assert_eq!(code, 200, "seed={seed}: healthy status returned {code}");
                assert_eq!(
                    serde_json::from_str::<ClusterStatus>(&body)?.node_id,
                    "node"
                );
                reached = true;
                break;
            }
        }
        assert!(reached, "seed={seed}: never reached cluster HTTP server");

        let mut rng = seed ^ 0x9e3779b97f4a7c15;
        for step in 1..=STEPS {
            let choice = draw(&mut rng) % 8;
            let bits = draw(&mut rng);
            match choice {
                0 => turmoil::partition("client", "node"),
                1 => turmoil::partition_oneway("client", "node"),
                2 => turmoil::repair("client", "node"),
                3 => {
                    if let Some((code, body)) = request("GET", "/raft/status", b"").await {
                        assert_eq!(code, 200, "seed={seed} step={step}: GET returned {code}");
                        let status: ClusterStatus = serde_json::from_str(&body)?;
                        assert_eq!(status.node_id, "node", "seed={seed} step={step}");
                        assert!(
                            status.commit_index <= status.log_length,
                            "seed={seed} step={step}: {status:?}"
                        );
                    }
                }
                4 => {
                    const INVALID: [&[u8]; 6] =
                        [b"{", b"[]", b"{}", b"null", b"{\"term\":\"bad\"}", b"\xff"];
                    let body = INVALID[((bits >> 8) % INVALID.len() as u64) as usize];
                    if let Some((code, _)) = request("POST", "/raft/append-entries", body).await {
                        assert_eq!(
                            code, 400,
                            "seed={seed} step={step}: malformed RPC returned {code}"
                        );
                    }
                }
                5 => {
                    let term = 1 + bits % 8;
                    let vote = RequestVoteRequest {
                        term,
                        candidate_id: format!("candidate-{}", (bits >> 8) % 3),
                        last_log_index: (bits >> 16) % 4,
                        last_log_term: (bits >> 24) % (term + 1),
                    };
                    let body = serde_json::to_vec(&vote)?;
                    if let Some((code, response)) =
                        request("POST", "/raft/request-vote", &body).await
                    {
                        assert_eq!(code, 200, "seed={seed} step={step}: vote returned {code}");
                        let _: RequestVoteResponse = serde_json::from_str(&response)?;
                    }
                }
                _ => {
                    let term = 1 + bits % 8;
                    let prev = (bits >> 8) % 3;
                    let append = AppendEntriesRequest {
                        term,
                        leader_id: "fuzz-leader".into(),
                        prev_log_index: prev,
                        prev_log_term: if prev == 0 { 0 } else { (bits >> 16) % 8 },
                        entries: if bits & 1 == 0 {
                            vec![]
                        } else {
                            vec![LogEntry {
                                term,
                                index: prev + 1,
                                key: if prev == 0 {
                                    RECEIPT.into()
                                } else {
                                    format!("fuzz/other-{prev}")
                                },
                                value: format!("attempted-rewrite-{bits}"),
                                extra: vec![],
                            }]
                        },
                        leader_commit: (bits >> 24) % 4,
                        peer_addrs: None,
                        learner_addrs: None,
                    };
                    let body = serde_json::to_vec(&append)?;
                    if let Some((code, response)) =
                        request("POST", "/raft/append-entries", &body).await
                    {
                        assert_eq!(code, 200, "seed={seed} step={step}: append returned {code}");
                        let _: AppendEntriesResponse = serde_json::from_str(&response)?;
                    }
                }
            }
            // Inspect the actual node even when the response was lost or partitioned.
            check_safety(&node, &mut last_term, seed, step).await;
        }
        turmoil::repair("client", "node");
        let mut recovered = false;
        for _ in 0..30 {
            if let Some((code, body)) = request("GET", "/raft/status", b"").await {
                assert_eq!(code, 200, "seed={seed}: repaired status returned {code}");
                assert_eq!(
                    serde_json::from_str::<ClusterStatus>(&body)?.node_id,
                    "node"
                );
                recovered = true;
                break;
            }
        }
        assert!(recovered, "seed={seed}: could not query after repair");
        check_safety(&node, &mut last_term, seed, STEPS + 1).await;
        tokio::time::sleep(Duration::from_millis(5)).await; // Let the host reap any panicked handler.
        Ok(())
    });
    sim.run()
        .unwrap_or_else(|e| panic!("turmoil fuzz seed={seed}: {e}"));
}

#[test]
fn fuzz_cluster_http_fault_sequences() {
    if let Ok(seed) = std::env::var("TURMOIL_FUZZ_SEED") {
        run_seed(
            seed.parse()
                .expect("TURMOIL_FUZZ_SEED must be a decimal u64"),
        );
    } else {
        let cases: u64 = std::env::var("TURMOIL_FUZZ_CASES")
            .map(|s| s.parse().expect("TURMOIL_FUZZ_CASES must be an integer"))
            .unwrap_or(32);
        assert!(
            (1..=1000).contains(&cases),
            "TURMOIL_FUZZ_CASES must be 1..=1000"
        );
        for i in 0..cases {
            run_seed(i.wrapping_mul(0x9e3779b97f4a7c15));
        }
    }
}
