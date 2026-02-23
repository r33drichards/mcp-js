//! Cluster write-forwarding simulation tests.
//!
//! Tests:
//!   1. Follower nodes forward writes to the leader via `put_or_forward`.
//!   2. Writes fail when the leader is down and no new leader is elected.
//!   3. After re-election, writes succeed again through the new leader.

use server::cluster::{ClusterConfig, ClusterNode, Role};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;

/// Pick N consecutive ports starting at `base`.
fn cluster_ports_3(base: u16) -> [u16; 3] {
    [base, base + 1, base + 2]
}

/// Build a `ClusterConfig` for node `idx` in a 3-node cluster.
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
        node_id: node_id.clone(),
        peers,
        peer_addrs,
        cluster_port: ports[idx],
        advertise_addr: Some(format!("127.0.0.1:{}", ports[idx])),
        heartbeat_interval: Duration::from_millis(80),
        election_timeout_min: Duration::from_millis(250),
        election_timeout_max: Duration::from_millis(500),
    }
}

fn temp_sled(label: &str) -> sled::Db {
    let dir = std::env::temp_dir().join(format!(
        "mcp-cluster-fwd-{}-{}-{}",
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
            let st = node.status().await;
            if st.role == Role::Leader {
                return Some(i);
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return None;
        }
        sleep(Duration::from_millis(200)).await;
    }
}

async fn wait_for_key(
    node: &Arc<ClusterNode>,
    key: &str,
    expected: &str,
    timeout: Duration,
) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if let Ok(Some(val)) = node.get(key).await {
            if val == expected {
                return true;
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        sleep(Duration::from_millis(200)).await;
    }
}

// ---------------------------------------------------------------------------
// Test 1: Followers forward writes to the leader
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn test_follower_write_forwarding() {
    let ports = cluster_ports_3(19400);

    // Start 3 nodes
    let mut nodes: Vec<Arc<ClusterNode>> = Vec::new();
    for i in 0..3 {
        let config = make_config_3(i, &ports);
        let db = temp_sled(&format!("fwd-node{}", i + 1));
        let node = ClusterNode::new(config, db);
        node.start().await;
        nodes.push(node);
    }

    // Wait for leader
    let leader_idx = wait_for_leader(&nodes, Duration::from_secs(15))
        .await
        .expect("No leader elected");

    println!("Leader is node{}", leader_idx + 1);

    // Find a follower
    let follower_idx = (leader_idx + 1) % 3;
    let follower = &nodes[follower_idx];

    println!("Writing through follower node{}", follower_idx + 1);

    // Write through the follower using put_or_forward
    follower
        .put_or_forward("forwarded-key".to_string(), "forwarded-value".to_string())
        .await
        .expect("put_or_forward from follower should succeed");

    // Verify the data is replicated to all nodes
    for (i, node) in nodes.iter().enumerate() {
        assert!(
            wait_for_key(node, "forwarded-key", "forwarded-value", Duration::from_secs(5)).await,
            "node{}: forwarded key not replicated",
            i + 1
        );
    }

    println!("Write forwarding from follower: PASSED");

    for node in &nodes {
        node.shutdown();
    }
    sleep(Duration::from_millis(200)).await;
}

// ---------------------------------------------------------------------------
// Test 2: Writes fail when leader is down (no quorum for re-election with
//         only 1 of 3 nodes alive)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn test_writes_fail_when_leader_down() {
    let ports = cluster_ports_3(19500);

    let mut nodes: Vec<Arc<ClusterNode>> = Vec::new();
    for i in 0..3 {
        let config = make_config_3(i, &ports);
        let db = temp_sled(&format!("down-node{}", i + 1));
        let node = ClusterNode::new(config, db);
        node.start().await;
        nodes.push(node);
    }

    let leader_idx = wait_for_leader(&nodes, Duration::from_secs(15))
        .await
        .expect("No leader elected");

    // Write some data to confirm the cluster works
    nodes[leader_idx]
        .put("before-crash".to_string(), "ok".to_string())
        .await
        .expect("Initial write should succeed");

    // Crash the leader and one follower (only 1 node alive = no quorum)
    let follower_to_crash = (leader_idx + 1) % 3;
    println!(
        "Crashing leader node{} and follower node{}",
        leader_idx + 1,
        follower_to_crash + 1
    );
    nodes[leader_idx].shutdown();
    nodes[follower_to_crash].shutdown();

    sleep(Duration::from_millis(500)).await;

    // The surviving node should not be able to write (no quorum)
    let survivor_idx = (leader_idx + 2) % 3;
    let survivor = &nodes[survivor_idx];

    // Try writing through the survivor – should fail
    // It might try to forward but leader is dead, or it might try to become
    // leader but can't get quorum.
    let result = survivor
        .put_or_forward("should-fail".to_string(), "nope".to_string())
        .await;

    assert!(
        result.is_err(),
        "Write should fail with no quorum: got {:?}",
        result
    );

    println!("Writes correctly fail when leader is down and no quorum: PASSED");

    // Cleanup
    for node in &nodes {
        node.shutdown();
    }
    sleep(Duration::from_millis(200)).await;
}

// ---------------------------------------------------------------------------
// Test 3: After re-election, writes succeed again
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn test_writes_resume_after_reelection() {
    let ports = cluster_ports_3(19600);

    let mut nodes: Vec<Arc<ClusterNode>> = Vec::new();
    for i in 0..3 {
        let config = make_config_3(i, &ports);
        let db = temp_sled(&format!("reelect-node{}", i + 1));
        let node = ClusterNode::new(config, db);
        node.start().await;
        nodes.push(node);
    }

    let leader_idx = wait_for_leader(&nodes, Duration::from_secs(15))
        .await
        .expect("No leader elected");

    // Write initial data
    nodes[leader_idx]
        .put("pre-crash".to_string(), "value1".to_string())
        .await
        .expect("Pre-crash write should succeed");

    println!("Crashing leader node{}", leader_idx + 1);
    nodes[leader_idx].shutdown();
    sleep(Duration::from_millis(500)).await;

    // The remaining 2 nodes should elect a new leader (2/3 = quorum)
    let survivors: Vec<Arc<ClusterNode>> = (0..3)
        .filter(|i| *i != leader_idx)
        .map(|i| nodes[i].clone())
        .collect();

    let new_leader_local = wait_for_leader(&survivors, Duration::from_secs(15))
        .await
        .expect("No new leader elected among survivors");

    let new_leader = &survivors[new_leader_local];
    let status = new_leader.status().await;
    println!("New leader: {} (term {})", status.node_id, status.term);

    // Write through the new leader
    new_leader
        .put("post-crash".to_string(), "value2".to_string())
        .await
        .expect("Post-crash write through new leader should succeed");

    // Write through a follower using put_or_forward
    let new_follower_local = if new_leader_local == 0 { 1 } else { 0 };
    let new_follower = &survivors[new_follower_local];

    new_follower
        .put_or_forward("forwarded-after-crash".to_string(), "value3".to_string())
        .await
        .expect("put_or_forward after re-election should succeed");

    // Verify all data on surviving nodes
    for node in &survivors {
        assert!(
            wait_for_key(node, "pre-crash", "value1", Duration::from_secs(5)).await,
            "pre-crash data lost"
        );
        assert!(
            wait_for_key(node, "post-crash", "value2", Duration::from_secs(5)).await,
            "post-crash write not replicated"
        );
        assert!(
            wait_for_key(node, "forwarded-after-crash", "value3", Duration::from_secs(5)).await,
            "forwarded write after crash not replicated"
        );
    }

    println!("Writes resume after re-election: PASSED");

    for node in &nodes {
        node.shutdown();
    }
    sleep(Duration::from_millis(200)).await;
}

// ---------------------------------------------------------------------------
// Test 4: Session log entries are replicated through Raft
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn test_session_log_replication_through_raft() {
    use server::mcp::session_log::{SessionLog, SessionLogEntry};

    let ports = cluster_ports_3(19700);

    let mut nodes: Vec<Arc<ClusterNode>> = Vec::new();
    for i in 0..3 {
        let config = make_config_3(i, &ports);
        let db = temp_sled(&format!("sesslog-node{}", i + 1));
        let node = ClusterNode::new(config, db);
        node.start().await;
        nodes.push(node);
    }

    let leader_idx = wait_for_leader(&nodes, Duration::from_secs(15))
        .await
        .expect("No leader elected");

    // Create a SessionLog backed by the leader's cluster node
    let local_db = temp_sled("sesslog-local");
    let session_log = SessionLog::from_config(
        sled::Config::new()
            .path(std::env::temp_dir().join(format!(
                "mcp-sesslog-test-{}-{}",
                std::process::id(),
                rand::random::<u32>()
            )))
            .temporary(true),
    )
    .unwrap()
    .with_cluster(nodes[leader_idx].clone());

    // Append a session log entry
    let entry = SessionLogEntry {
        input_heap: None,
        output_heap: "abc123".to_string(),
        code: "console.log('hello')".to_string(),
        timestamp: "2025-01-01T00:00:00Z".to_string(),
    };

    session_log
        .append("test-session", entry)
        .await
        .expect("Session log append should succeed");

    // Wait for replication
    sleep(Duration::from_secs(2)).await;

    // Verify session data is in the Raft data tree on all nodes
    for (i, node) in nodes.iter().enumerate() {
        let sessions = node.scan_prefix("sl:s:").unwrap();
        assert!(
            !sessions.is_empty(),
            "node{}: no session keys found in Raft data tree",
            i + 1
        );

        let entries = node.scan_prefix("sl:e:test-session:").unwrap();
        assert!(
            !entries.is_empty(),
            "node{}: no entry keys found in Raft data tree",
            i + 1
        );
    }

    // Read sessions through the session log (connected to a follower)
    let follower_idx = (leader_idx + 1) % 3;
    let follower_log = SessionLog::from_config(
        sled::Config::new()
            .path(std::env::temp_dir().join(format!(
                "mcp-sesslog-follower-{}-{}",
                std::process::id(),
                rand::random::<u32>()
            )))
            .temporary(true),
    )
    .unwrap()
    .with_cluster(nodes[follower_idx].clone());

    let sessions = follower_log
        .list_sessions()
        .await
        .expect("list_sessions from follower should succeed");
    assert!(
        sessions.contains(&"test-session".to_string()),
        "session name not found on follower: {:?}",
        sessions
    );

    let entries = follower_log
        .list_entries("test-session", None)
        .await
        .expect("list_entries from follower should succeed");
    assert_eq!(entries.len(), 1, "expected 1 entry, got {}", entries.len());

    println!("Session log replication through Raft: PASSED");

    drop(local_db);
    for node in &nodes {
        node.shutdown();
    }
    sleep(Duration::from_millis(200)).await;
}

// ---------------------------------------------------------------------------
// Test 5: Dynamic peer join
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn test_dynamic_peer_join() {
    use server::cluster::JoinRequest;

    // Start a 2-node cluster, then dynamically add a 3rd node.
    let ports: [u16; 3] = [19800, 19801, 19802];

    // Initially only start node1 and node2
    let config1 = {
        let mut pa = HashMap::new();
        pa.insert("node2".to_string(), format!("127.0.0.1:{}", ports[1]));
        ClusterConfig {
            node_id: "node1".to_string(),
            peers: vec![format!("127.0.0.1:{}", ports[1])],
            peer_addrs: pa,
            cluster_port: ports[0],
            advertise_addr: Some(format!("127.0.0.1:{}", ports[0])),
            heartbeat_interval: Duration::from_millis(80),
            election_timeout_min: Duration::from_millis(250),
            election_timeout_max: Duration::from_millis(500),
        }
    };
    let config2 = {
        let mut pa = HashMap::new();
        pa.insert("node1".to_string(), format!("127.0.0.1:{}", ports[0]));
        ClusterConfig {
            node_id: "node2".to_string(),
            peers: vec![format!("127.0.0.1:{}", ports[0])],
            peer_addrs: pa,
            cluster_port: ports[1],
            advertise_addr: Some(format!("127.0.0.1:{}", ports[1])),
            heartbeat_interval: Duration::from_millis(80),
            election_timeout_min: Duration::from_millis(250),
            election_timeout_max: Duration::from_millis(500),
        }
    };

    let db1 = temp_sled("dyn-join-node1");
    let db2 = temp_sled("dyn-join-node2");

    let node1 = ClusterNode::new(config1, db1);
    let node2 = ClusterNode::new(config2, db2);
    node1.start().await;
    node2.start().await;

    let initial_nodes = vec![node1.clone(), node2.clone()];

    // Wait for leader among the initial 2 nodes
    let leader_idx = wait_for_leader(&initial_nodes, Duration::from_secs(15))
        .await
        .expect("No leader elected in initial 2-node cluster");
    let leader = &initial_nodes[leader_idx];

    println!("Initial leader: node{}", leader_idx + 1);

    // Write some data before node3 joins
    leader
        .put("before-join".to_string(), "initial".to_string())
        .await
        .expect("Initial write should succeed");

    // Now start node3 with no seed peers, and have it join via the leader
    let config3 = ClusterConfig {
        node_id: "node3".to_string(),
        peers: vec![], // no seed peers
        peer_addrs: HashMap::new(),
        cluster_port: ports[2],
        advertise_addr: Some(format!("127.0.0.1:{}", ports[2])),
        heartbeat_interval: Duration::from_millis(80),
        election_timeout_min: Duration::from_millis(250),
        election_timeout_max: Duration::from_millis(500),
    };
    let db3 = temp_sled("dyn-join-node3");
    let node3 = ClusterNode::new(config3, db3);
    node3.start().await;

    // Send join request to the leader
    leader
        .handle_join(JoinRequest {
            node_id: "node3".to_string(),
            addr: format!("127.0.0.1:{}", ports[2]),
        })
        .await
        .expect("Join should succeed on leader");

    // Also tell node3 about the existing peers so it can participate
    {
        let mut state = node3.state.write().await;
        state.peers.push(format!("127.0.0.1:{}", ports[0]));
        state.peers.push(format!("127.0.0.1:{}", ports[1]));
        state.peer_addrs.insert("node1".to_string(), format!("127.0.0.1:{}", ports[0]));
        state.peer_addrs.insert("node2".to_string(), format!("127.0.0.1:{}", ports[1]));
    }

    // Wait for data to replicate to node3
    assert!(
        wait_for_key(&node3, "before-join", "initial", Duration::from_secs(10)).await,
        "node3: pre-join data not replicated after joining"
    );

    // Write new data and verify it reaches node3
    leader
        .put("after-join".to_string(), "new-data".to_string())
        .await
        .expect("Post-join write should succeed");

    assert!(
        wait_for_key(&node3, "after-join", "new-data", Duration::from_secs(5)).await,
        "node3: post-join data not replicated"
    );

    // Wait a bit for node3 to learn leader_id via heartbeats
    sleep(Duration::from_secs(2)).await;

    // Verify node3 can forward writes back through the cluster
    node3
        .put_or_forward("from-node3".to_string(), "hello".to_string())
        .await
        .expect("put_or_forward from dynamically joined node3 should succeed");

    for node in &[&node1, &node2] {
        assert!(
            wait_for_key(node, "from-node3", "hello", Duration::from_secs(5)).await,
            "data from node3 not replicated"
        );
    }

    println!("Dynamic peer join: PASSED");

    for node in &[&node1, &node2, &node3] {
        node.shutdown();
    }
    sleep(Duration::from_millis(200)).await;
}

// ---------------------------------------------------------------------------
// Test 6: Dynamic peer leave
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn test_dynamic_peer_leave() {
    use server::cluster::LeaveRequest;

    let ports = cluster_ports_3(19900);

    let mut nodes: Vec<Arc<ClusterNode>> = Vec::new();
    for i in 0..3 {
        let config = make_config_3(i, &ports);
        let db = temp_sled(&format!("dyn-leave-node{}", i + 1));
        let node = ClusterNode::new(config, db);
        node.start().await;
        nodes.push(node);
    }

    let leader_idx = wait_for_leader(&nodes, Duration::from_secs(15))
        .await
        .expect("No leader elected");

    let leader = &nodes[leader_idx];
    println!("Leader: node{}", leader_idx + 1);

    // Write data with all 3 nodes
    leader
        .put("pre-leave".to_string(), "ok".to_string())
        .await
        .expect("Pre-leave write should succeed");

    // Remove a follower from the cluster
    let leaving_idx = (leader_idx + 1) % 3;
    let leaving_id = format!("node{}", leaving_idx + 1);

    leader
        .handle_leave(LeaveRequest {
            node_id: leaving_id.clone(),
        })
        .await
        .expect("Leave should succeed");

    println!("Removed {} from cluster", leaving_id);

    // Shut down the leaving node
    nodes[leaving_idx].shutdown();
    sleep(Duration::from_millis(500)).await;

    // The remaining 2 nodes should still be able to write (quorum = 2/2)
    leader
        .put("post-leave".to_string(), "still-working".to_string())
        .await
        .expect("Post-leave write should succeed with 2-node cluster");

    // Verify on the other surviving node
    let other_idx = (leader_idx + 2) % 3;
    assert!(
        wait_for_key(
            &nodes[other_idx],
            "post-leave",
            "still-working",
            Duration::from_secs(5)
        )
        .await,
        "Post-leave data not replicated"
    );

    // Verify the leader's peer set no longer contains the leaving node
    let status = leader.status().await;
    assert!(
        !status.peer_addrs.contains_key(&leaving_id),
        "leaving node should not be in peer_addrs: {:?}",
        status.peer_addrs
    );

    println!("Dynamic peer leave: PASSED");

    for node in &nodes {
        node.shutdown();
    }
    sleep(Duration::from_millis(200)).await;
}
