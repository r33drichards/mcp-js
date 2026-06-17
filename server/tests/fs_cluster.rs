//! Cluster coordination for fs label writes: pointer moves route through the
//! Raft leader so CAS is linearizable — two followers racing a push see exactly
//! one winner; the loser is rejected and must rebase.

use server::cluster::{ClusterConfig, ClusterNode, Role};
use server::engine::fs_labels::LabelStore;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;

fn make_config_3(idx: usize, ports: &[u16; 3]) -> ClusterConfig {
    let node_id = format!("node{}", idx + 1);
    let peers: Vec<String> = ports
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != idx)
        .map(|(_, p)| format!("127.0.0.1:{}", p))
        .collect();
    let peer_addrs: HashMap<String, String> = ports
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != idx)
        .map(|(i, p)| (format!("node{}", i + 1), format!("127.0.0.1:{}", p)))
        .collect();
    ClusterConfig {
        node_id,
        peers,
        peer_addrs,
        cluster_port: ports[idx],
        advertise_addr: Some(format!("127.0.0.1:{}", ports[idx])),
        heartbeat_interval: Duration::from_millis(80),
        election_timeout_min: Duration::from_millis(250),
        election_timeout_max: Duration::from_millis(500),
        learner: false,
    }
}

fn temp_sled(label: &str) -> sled::Db {
    let dir = std::env::temp_dir().join(format!(
        "mcp-fs-cluster-{}-{}-{}",
        label,
        std::process::id(),
        rand::random::<u32>()
    ));
    sled::Config::new().path(dir).temporary(true).open().unwrap()
}

async fn wait_for_leader(nodes: &[Arc<ClusterNode>], timeout: Duration) -> Option<usize> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        for (i, node) in nodes.iter().enumerate() {
            if node.status().await.role == Role::Leader {
                return Some(i);
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return None;
        }
        sleep(Duration::from_millis(150)).await;
    }
}

"multi_thread"
async fn two_followers_race_a_push_exactly_one_wins() {
    let ports = [19540u16, 19541, 19542];
    let mut nodes: Vec<Arc<ClusterNode>> = Vec::new();
    for i in 0..3 {
        let node = ClusterNode::new(make_config_3(i, &ports), temp_sled(&format!("n{i}")));
        node.start().await;
        nodes.push(node);
    }
    let leader = wait_for_leader(&nodes, Duration::from_secs(20))
        .await
        .expect("no leader elected");

        let stores: Vec<LabelStore> = nodes
        .iter()
        .map(|n| LabelStore::in_memory().with_cluster(n.clone()))
        .collect();

    let c0 = [0u8; 32];
    let c1 = [1u8; 32];
    let c2 = [2u8; 32];

        stores[leader].create("main", c0, None).await.expect("create main");
    let f1 = (leader + 1) % 3;
    let f2 = (leader + 2) % 3;
    for f in [f1, f2] {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            if stores[f].resolve("main").await.ok().flatten() == Some(c0) {
                break;
            }
            assert!(tokio::time::Instant::now() < deadline, "follower never saw c0");
            sleep(Duration::from_millis(100)).await;
        }
    }

        let s1 = stores[f1].clone();
    let s2 = stores[f2].clone();
    let (r1, r2) = tokio::join!(
        async move { s1.cas("main", Some(c0), c1, None).await },
        async move { s2.cas("main", Some(c0), c2, None).await },
    );
    let r1 = r1.expect("cas 1 ok");
    let r2 = r2.expect("cas 2 ok");

        assert!(r1 ^ r2, "exactly one CAS must win (r1={r1}, r2={r2})");

            let winner = if r1 { c1 } else { c2 };
    let loser_store = if r1 { &stores[f2] } else { &stores[f1] };
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if loser_store.resolve("main").await.ok().flatten() == Some(winner) {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "loser never observed the winning head"
        );
        sleep(Duration::from_millis(100)).await;
    }

        let loser_new = if r1 { c2 } else { c1 };
    assert!(
        !loser_store.cas("main", Some(c0), loser_new, None).await.unwrap(),
        "stale-expected CAS must be rejected"
    );
    assert!(
        loser_store.cas("main", Some(winner), loser_new, None).await.unwrap(),
        "rebased CAS onto the current head should succeed"
    );

    for n in &nodes {
        n.shutdown();
    }
}

/// The combined atomic write (specs/FsLabelAtomicWrite): each label move
/// replicates the head pointer AND its reflog entry in one Raft entry, so a
/// follower that observes the moved head also observes the full reflog — the
/// head is never replicated ahead of the reflog that records it.
"multi_thread"
async fn head_and_reflog_commit_together_across_nodes() {
    let ports = [19543u16, 19544, 19545];
    let mut nodes: Vec<Arc<ClusterNode>> = Vec::new();
    for i in 0..3 {
        let node = ClusterNode::new(make_config_3(i, &ports), temp_sled(&format!("r{i}")));
        node.start().await;
        nodes.push(node);
    }
    let leader = wait_for_leader(&nodes, Duration::from_secs(20))
        .await
        .expect("no leader elected");
    let stores: Vec<LabelStore> = nodes
        .iter()
        .map(|n| LabelStore::in_memory().with_cluster(n.clone()))
        .collect();

    let c0 = [0u8; 32];
    let c1 = [1u8; 32];

        stores[leader].create("main", c0, Some("genesis".into())).await.unwrap();
    assert!(stores[leader]
        .cas("main", Some(c0), c1, Some("advance".into()))
        .await
        .unwrap());

            let follower = (leader + 1) % 3;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if stores[follower].resolve("main").await.ok().flatten() == Some(c1) {
            let log = stores[follower].log("main").await.unwrap();
            assert_eq!(log.len(), 2, "follower saw head=c1 but reflog is incomplete");
            assert_eq!(log[0].to, c0);
            assert_eq!(log[0].message.as_deref(), Some("genesis"));
            assert_eq!(log[1].to, c1);
            assert_eq!(log[1].message.as_deref(), Some("advance"));
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "follower never saw c1");
        sleep(Duration::from_millis(100)).await;
    }

    for n in &nodes {
        n.shutdown();
    }
}
