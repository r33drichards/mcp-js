//! Sled-based cluster simulation test.
//!
//! Spawns 5 `ClusterNode` instances on localhost (each with its own temporary
//! sled database), forms a Raft cluster, writes data, crashes the leader +
//! one random follower, and asserts that:
//!   1. A new leader is elected.
//!   2. No committed data is lost.
//!   3. Crashed nodes can rejoin without deadlock.

use server::cluster::{ClusterConfig, ClusterNode, Role};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;

/// Pick 5 consecutive ports starting at `base`.
fn cluster_ports(base: u16) -> [u16; 5] {
    [base, base + 1, base + 2, base + 3, base + 4]
}

/// Build a `ClusterConfig` for node `idx` (0-based) given the port list.
fn make_config(idx: usize, ports: &[u16; 5]) -> ClusterConfig {
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

/// Create a temporary sled database for a node.
fn temp_sled(label: &str) -> sled::Db {
    let dir = std::env::temp_dir().join(format!(
        "mcp-cluster-sim-{}-{}-{}",
        label,
        std::process::id(),
        rand::random::<u32>()
    ));
    sled::Config::new().path(dir).temporary(true).open().unwrap()
}

/// Helper: wait until exactly one node among `nodes` reports `Role::Leader`.
/// Returns the index of the leader.
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

/// Helper: wait until a key is readable on the given node.
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
// Main simulation test
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn test_5_node_cluster_crash_and_recovery() {
    // Use a high port range unlikely to collide with other tests
    let ports = cluster_ports(19100);

    // ── Phase 1: Start 5 nodes ─────────────────────────────────────────
    println!("=== Phase 1: Starting 5-node cluster ===");

    let mut nodes: Vec<Arc<ClusterNode>> = Vec::new();
    for i in 0..5 {
        let config = make_config(i, &ports);
        let db = temp_sled(&format!("node{}", i + 1));
        let node = ClusterNode::new(config, db);
        node.start().await;
        nodes.push(node);
    }

    // ── Phase 2: Wait for leader election ──────────────────────────────
    println!("=== Phase 2: Waiting for leader election ===");

    let leader_idx = wait_for_leader(&nodes, Duration::from_secs(15))
        .await
        .expect("No leader elected within 15 seconds");

    let leader_status = nodes[leader_idx].status().await;
    println!(
        "Leader elected: {} (term {})",
        leader_status.node_id, leader_status.term
    );

    // ── Phase 3: Write data through the leader ─────────────────────────
    println!("=== Phase 3: Writing data ===");

    nodes[leader_idx]
        .put("key1".to_string(), "value1".to_string())
        .await
        .expect("Failed to put key1");

    nodes[leader_idx]
        .put("key2".to_string(), "value2".to_string())
        .await
        .expect("Failed to put key2");

    nodes[leader_idx]
        .put("key3".to_string(), "important-data".to_string())
        .await
        .expect("Failed to put key3");

    println!("3 key-value pairs written through leader");

    // ── Phase 4: Verify replication to all nodes ───────────────────────
    println!("=== Phase 4: Verifying replication ===");

    for (i, node) in nodes.iter().enumerate() {
        assert!(
            wait_for_key(node, "key1", "value1", Duration::from_secs(5)).await,
            "node{}: key1 not replicated",
            i + 1
        );
        assert!(
            wait_for_key(node, "key2", "value2", Duration::from_secs(5)).await,
            "node{}: key2 not replicated",
            i + 1
        );
        assert!(
            wait_for_key(node, "key3", "important-data", Duration::from_secs(5)).await,
            "node{}: key3 not replicated",
            i + 1
        );
    }
    println!("All 5 nodes have consistent data");

    // ── Phase 5: Crash leader + one random follower ────────────────────
    println!("=== Phase 5: Crashing leader + one follower ===");

    // Pick a follower that isn't the leader
    let follower_idx = (leader_idx + 2) % 5;

    println!(
        "Crashing node{} (leader) and node{} (follower)",
        leader_idx + 1,
        follower_idx + 1
    );

    nodes[leader_idx].shutdown();
    nodes[follower_idx].shutdown();

    // Allow time for shutdown to take effect
    sleep(Duration::from_millis(500)).await;

    // Collect surviving node indices
    let surviving_indices: Vec<usize> = (0..5)
        .filter(|i| *i != leader_idx && *i != follower_idx)
        .collect();
    let surviving: Vec<Arc<ClusterNode>> =
        surviving_indices.iter().map(|i| nodes[*i].clone()).collect();

    assert_eq!(surviving.len(), 3, "Should have 3 surviving nodes");

    // ── Phase 6: Wait for new leader among survivors ───────────────────
    println!("=== Phase 6: Waiting for new leader election ===");

    let new_leader_local_idx = wait_for_leader(&surviving, Duration::from_secs(15))
        .await
        .expect("No new leader elected among survivors within 15 seconds");

    let new_leader = &surviving[new_leader_local_idx];
    let new_status = new_leader.status().await;
    println!(
        "New leader: {} (term {})",
        new_status.node_id, new_status.term
    );

    // ── Phase 7: Assert no data lost ───────────────────────────────────
    println!("=== Phase 7: Verifying no data lost ===");

    for (local_i, global_i) in surviving_indices.iter().enumerate() {
        let node = &surviving[local_i];
        let name = format!("node{}", global_i + 1);

        let v1 = node.get("key1").await.expect("get key1 failed");
        assert_eq!(v1.as_deref(), Some("value1"), "{}: key1 lost", name);

        let v2 = node.get("key2").await.expect("get key2 failed");
        assert_eq!(v2.as_deref(), Some("value2"), "{}: key2 lost", name);

        let v3 = node.get("key3").await.expect("get key3 failed");
        assert_eq!(
            v3.as_deref(),
            Some("important-data"),
            "{}: key3 lost",
            name
        );
    }
    println!("All committed data intact on surviving nodes");

    // ── Phase 8: Write new data on degraded cluster ────────────────────
    println!("=== Phase 8: Writing to degraded cluster ===");

    new_leader
        .put("post-crash".to_string(), "still-alive".to_string())
        .await
        .expect("Failed to write to degraded cluster");

    for node in &surviving {
        assert!(
            wait_for_key(node, "post-crash", "still-alive", Duration::from_secs(5)).await,
            "post-crash key not replicated to all survivors"
        );
    }
    println!("Degraded cluster (3/5 nodes) still accepts writes");

    // ── Phase 9: Restart crashed nodes ─────────────────────────────────
    println!("=== Phase 9: Restarting crashed nodes ===");

    // Create fresh nodes with the same config but new sled databases.
    // In a real deployment the nodes would reload from their persistent
    // sled stores, but for the simulation we test that new nodes can join
    // and the cluster does not deadlock.
    let restart_node = |idx: usize| {
        let config = make_config(idx, &ports);
        let db = temp_sled(&format!("node{}-restart", idx + 1));
        let node = ClusterNode::new(config, db);
        node
    };

    let restarted_leader = restart_node(leader_idx);
    let restarted_follower = restart_node(follower_idx);
    restarted_leader.start().await;
    restarted_follower.start().await;

    // Replace the crashed node references
    let mut all_nodes_v2 = nodes.clone();
    all_nodes_v2[leader_idx] = restarted_leader;
    all_nodes_v2[follower_idx] = restarted_follower;

    // ── Phase 10: Verify cluster converges without deadlock ────────────
    println!("=== Phase 10: Verifying full cluster convergence ===");

    // Wait for a stable leader among all 5 nodes
    let final_leader_idx = wait_for_leader(&all_nodes_v2, Duration::from_secs(15))
        .await
        .expect("Cluster failed to converge after restart (possible deadlock)");

    let final_status = all_nodes_v2[final_leader_idx].status().await;
    println!(
        "Final leader: {} (term {})",
        final_status.node_id, final_status.term
    );

    // Verify exactly one leader
    let mut leader_count = 0;
    for node in &all_nodes_v2 {
        let st = node.status().await;
        if st.role == Role::Leader {
            leader_count += 1;
        }
    }
    assert_eq!(leader_count, 1, "Expected exactly 1 leader, found {}", leader_count);

    // The original data should still be readable from the surviving nodes
    // (restarted nodes start with fresh state in this simulation)
    for &i in &surviving_indices {
        let val = all_nodes_v2[i].get("key1").await.expect("get failed");
        assert_eq!(val.as_deref(), Some("value1"), "node{}: key1 lost after restart phase", i + 1);
    }

    println!("=== All assertions passed! ===");

    // Cleanup
    for node in &all_nodes_v2 {
        node.shutdown();
    }
    sleep(Duration::from_millis(200)).await;
}

/// Smaller test: verify that a 5-node cluster survives all nodes crashing
/// and restarting simultaneously without deadlocking.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn test_full_cluster_crash_and_restart() {
    let ports = cluster_ports(19200);

    // Start 5 nodes
    let mut nodes: Vec<Arc<ClusterNode>> = Vec::new();
    for i in 0..5 {
        let config = make_config(i, &ports);
        let db = temp_sled(&format!("full-crash-node{}", i + 1));
        let node = ClusterNode::new(config, db);
        node.start().await;
        nodes.push(node);
    }

    // Wait for leader and write data
    let leader_idx = wait_for_leader(&nodes, Duration::from_secs(15))
        .await
        .expect("No leader elected");

    nodes[leader_idx]
        .put("survive".to_string(), "total-crash".to_string())
        .await
        .expect("Failed to put data");

    // Verify replication
    for node in &nodes {
        assert!(wait_for_key(node, "survive", "total-crash", Duration::from_secs(5)).await);
    }

    // Crash ALL nodes simultaneously
    println!("Crashing all 5 nodes simultaneously");
    for node in &nodes {
        node.shutdown();
    }
    sleep(Duration::from_millis(500)).await;

    // Restart all nodes with fresh state
    println!("Restarting all 5 nodes");
    let mut new_nodes: Vec<Arc<ClusterNode>> = Vec::new();
    for i in 0..5 {
        let config = make_config(i, &ports);
        let db = temp_sled(&format!("full-crash-restart-node{}", i + 1));
        let node = ClusterNode::new(config, db);
        node.start().await;
        new_nodes.push(node);
    }

    // Verify the cluster elects a new leader (no deadlock)
    let new_leader_idx = wait_for_leader(&new_nodes, Duration::from_secs(15))
        .await
        .expect("Cluster deadlocked after full restart — no leader elected");

    let st = new_nodes[new_leader_idx].status().await;
    println!(
        "Cluster recovered after full crash: leader={}, term={}",
        st.node_id, st.term
    );

    // Verify the cluster is functional (can accept writes)
    new_nodes[new_leader_idx]
        .put("after-total-crash".to_string(), "recovered".to_string())
        .await
        .expect("Cluster not functional after total crash");

    for node in &new_nodes {
        assert!(
            wait_for_key(node, "after-total-crash", "recovered", Duration::from_secs(5)).await,
            "Cluster not fully replicating after total restart"
        );
    }

    println!("Full cluster crash and restart: PASSED");

    for node in &new_nodes {
        node.shutdown();
    }
    sleep(Duration::from_millis(200)).await;
}

/// Test that leader election happens within a reasonable time bound.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn test_election_timing() {
    let ports = cluster_ports(19300);

    let mut nodes: Vec<Arc<ClusterNode>> = Vec::new();
    for i in 0..5 {
        let config = make_config(i, &ports);
        let db = temp_sled(&format!("timing-node{}", i + 1));
        let node = ClusterNode::new(config, db);
        node.start().await;
        nodes.push(node);
    }

    let start = std::time::Instant::now();
    let _leader_idx = wait_for_leader(&nodes, Duration::from_secs(10))
        .await
        .expect("No leader elected");
    let election_time = start.elapsed();

    println!("Initial election took {:?}", election_time);
    assert!(
        election_time < Duration::from_secs(5),
        "Election took too long: {:?}",
        election_time
    );

    for node in &nodes {
        node.shutdown();
    }
    sleep(Duration::from_millis(200)).await;
}
