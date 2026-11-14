//! Three-node Raft simulations over turmoil TCP.
#[path = "support/peer_sim.rs"]
mod peer_sim;
use peer_sim::run_cluster;
use scheduler_raft_audit::cluster::Role;
use std::time::Duration;

#[test]
fn three_nodes_elect_over_simulated_peer_connections() {
    run_cluster(7, |shared| async move {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let nodes = shared.borrow().clone();
        assert_eq!(
            nodes.len(),
            3,
            "every node must bind before the election assertion"
        );
        let mut leaders = Vec::new();
        let mut statuses = Vec::new();
        for (name, node) in &nodes {
            let status = node.status().await;
            if status.role == Role::Leader {
                leaders.push((name.clone(), node.clone()));
            }
            statuses.push((name.clone(), status));
        }
        assert_eq!(
            leaders.len(),
            1,
            "peer votes must produce exactly one leader: {statuses:?}"
        );
        let (name, leader) = &leaders[0];
        assert_eq!(name, "n0", "shorter election timer should win");
        tokio::time::timeout(
            Duration::from_secs(1),
            leader.put("simulated-replication".into(), "committed".into()),
        )
        .await
        .expect("leader proposal timed out")
        .expect("majority must commit through turmoil TCP");
        let mut replicated = false;
        for _ in 0..30 {
            let mut count = 0;
            for node in nodes.values() {
                if node.get("simulated-replication").await.unwrap().as_deref() == Some("committed")
                {
                    count += 1;
                }
            }
            if count == 3 {
                replicated = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            replicated,
            "all three nodes must apply the majority-committed value"
        );
        Ok(())
    });
}

#[test]
fn majority_replaces_isolated_leader_and_recovers_after_repair() {
    run_cluster(17, |shared| async move {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let nodes = shared.borrow().clone();
        assert_eq!(nodes.len(), 3);
        let old = nodes["n0"].clone();
        assert_eq!(old.status().await.role, Role::Leader);
        tokio::time::timeout(
            Duration::from_secs(1),
            old.put("baseline".into(), "v0".into()),
        )
        .await
        .expect("baseline proposal timed out")
        .expect("baseline commit");
        turmoil::partition("n0", "n1");
        turmoil::partition("n0", "n2");
        tokio::time::sleep(Duration::from_millis(400)).await;
        let new = nodes["n1"].clone();
        let status = new.status().await;
        let all = vec![
            nodes["n0"].status().await,
            status.clone(),
            nodes["n2"].status().await,
        ];
        assert_eq!(
            status.role,
            Role::Leader,
            "majority must elect a new leader: {all:?}"
        );
        let isolated = tokio::time::timeout(
            Duration::from_millis(200),
            old.put("isolated".into(), "must-not-commit".into()),
        )
        .await;
        assert!(
            !matches!(isolated, Ok(Ok(()))),
            "isolated minority must not acknowledge commit"
        );
        tokio::time::timeout(
            Duration::from_secs(1),
            new.put("after-partition".into(), "v1".into()),
        )
        .await
        .expect("majority proposal timed out")
        .expect("majority commit");
        turmoil::repair("n0", "n1");
        turmoil::repair("n0", "n2");
        let mut converged = false;
        for _ in 0..40 {
            let mut applied = 0;
            for node in nodes.values() {
                if node.get("after-partition").await?.as_deref() == Some("v1") {
                    applied += 1;
                }
            }
            if applied == 3 {
                converged = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            converged,
            "repaired minority must apply the new majority commit"
        );
        assert_eq!(
            old.get("isolated").await?.as_deref(),
            None,
            "the uncommitted minority proposal must be discarded"
        );
        Ok(())
    });
}

#[test]
fn fuzz_input_drives_full_peer_network() {
    // Identical entry point to the libFuzzer target, with faulted and healthy seeds.
    for input in [
        &[0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 7, 6, 2, 3][..],
        &[7, 0, 0, 0, 0, 0, 0, 0, 4, 5, 7, 6, 2, 3][..],
    ] {
        peer_sim::run_fault_case(input);
    }
}
