//! Learner (non-voting member) tests.
//!
//! A learner replicates the log but is excluded from election and commit
//! quorums. This is what makes ephemeral nodes safe: a learner joining or
//! dying must never affect the cluster's ability to elect a leader or commit
//! writes. The decisive test here is that killing a learner does not block
//! the leader's writes — which it would if the learner were a voter.

use server::cluster::{ClusterConfig, ClusterNode, Role};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;

fn temp_sled(label: &str) -> sled::Db {
    let dir = std::env::temp_dir().join(format!(
        "mcp-cluster-learner-{}-{}-{}",
        label,
        std::process::id(),
        rand::random::<u32>()
    ));
    sled::Config::new()
        .path(dir)
        .temporary(true)
        .open()
        .unwrap()
}

fn base_config(node_id: &str, cluster_port: u16) -> ClusterConfig {
    ClusterConfig {
        node_id: node_id.to_string(),
        peers: Vec::new(),
        peer_addrs: HashMap::new(),
        cluster_port,
        advertise_addr: Some(format!("127.0.0.1:{}", cluster_port)),
        heartbeat_interval: Duration::from_millis(80),
        election_timeout_min: Duration::from_millis(250),
        election_timeout_max: Duration::from_millis(500),
        learner: false,
    }
}

async fn wait_for_leader(node: &Arc<ClusterNode>, timeout: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        if node.status().await.role == Role::Leader {
            return true;
        }
        sleep(Duration::from_millis(50)).await;
    }
    false
}


async fn learner_replicates_but_does_not_break_quorum() {
    let p1 = 47011;
    let p2 = 47012;
    let addr2 = format!("127.0.0.1:{}", p2);

            let node1 = ClusterNode::new(base_config("node1", p1), temp_sled("voter"));

        let mut cfg2 = base_config("node2", p2);
    cfg2.peers = vec![format!("127.0.0.1:{}", p1)];
    cfg2.peer_addrs
        .insert("node1".to_string(), format!("127.0.0.1:{}", p1));
    cfg2.learner = true;
    let node2 = ClusterNode::new(cfg2, temp_sled("learner"));

    node1.start().await;
    node2.start().await;

    assert!(
        wait_for_leader(&node1, Duration::from_secs(10)).await,
        "the sole voter should become leader"
    );

        node1
        .add_peer("node2".to_string(), addr2.clone(), true)
        .await
        .expect("leader adds learner");

            node1
        .put("k1".to_string(), "v1".to_string())
        .await
        .expect("write commits with a single voter");

        let mut replicated = false;
    for _ in 0..40 {
        if node2.get("k1").await.unwrap().as_deref() == Some("v1") {
            replicated = true;
            break;
        }
        sleep(Duration::from_millis(50)).await;
    }
    assert!(replicated, "learner should replicate committed entries");

        let st2 = node2.status().await;
    assert_eq!(st2.role, Role::Follower, "a learner stays a follower");
    assert!(st2.is_learner, "node2 reports itself as a learner");

                    node2.shutdown();
    sleep(Duration::from_millis(200)).await;

    node1
        .put("k2".to_string(), "v2".to_string())
        .await
        .expect("write still commits after the learner dies");

    assert_eq!(node1.get("k2").await.unwrap().as_deref(), Some("v2"));
    assert_eq!(
        node1.status().await.role,
        Role::Leader,
        "leader is retained"
    );

    node1.shutdown();
}
