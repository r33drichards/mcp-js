#[path = "../../src/cluster.rs"]
pub mod cluster;

#[cfg(test)]
mod tests {
    use super::cluster::*;
    use std::{collections::HashMap, time::Duration};

    fn node() -> std::sync::Arc<ClusterNode> {
        ClusterNode::new(
            ClusterConfig {
                node_id: "old-leader".into(),
                peers: vec![],
                peer_addrs: HashMap::new(),
                cluster_port: 0,
                advertise_addr: None,
                heartbeat_interval: Duration::from_millis(50),
                election_timeout_min: Duration::from_millis(100),
                election_timeout_max: Duration::from_millis(200),
                learner: false,
            },
            sled::Config::default().temporary(true).open().unwrap(),
        )
    }

    #[tokio::test]
    async fn replaced_claim_must_not_report_committed() {
        let node = node();
        {
            let mut state = node.state.write().await;
            state.role = Role::Leader;
            state.current_term = 1;
            state.leader_id = Some("old-leader".into());
        }
        let proposer = node.clone();
        let proposal = tokio::spawn(async move {
            proposer
                .cas("schedule:claim".into(), None, "old-owner".into())
                .await
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if node.state.read().await.log.len() == 1 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let response = node
            .handle_append_entries(AppendEntriesRequest {
                term: 2,
                leader_id: "new-leader".into(),
                prev_log_index: 0,
                prev_log_term: 0,
                entries: vec![LogEntry {
                    term: 2,
                    index: 1,
                    key: "schedule:claim".into(),
                    value: "new-owner".into(),
                    extra: vec![],
                }],
                leader_commit: 1,
                peer_addrs: None,
                learner_addrs: None,
            })
            .await;
        assert!(response.success);
        assert_eq!(
            node.get("schedule:claim").await.unwrap(),
            Some("new-owner".into())
        );
        let result = proposal.await.unwrap();
        assert_ne!(
            result,
            Ok(true),
            "old owner was told its replaced claim committed"
        );
    }

    #[tokio::test]
    async fn committed_receipt_must_not_be_truncated_by_inconsistent_prefix() {
        let node = node();
        let response = node
            .handle_append_entries(AppendEntriesRequest {
                term: 1,
                leader_id: "leader-1".into(),
                prev_log_index: 0,
                prev_log_term: 0,
                entries: vec![LogEntry {
                    term: 1,
                    index: 1,
                    key: "schedule:claim".into(),
                    value: "consumed".into(),
                    extra: vec![],
                }],
                leader_commit: 1,
                peer_addrs: None,
                learner_addrs: None,
            })
            .await;
        assert!(response.success);
        let response = node
            .handle_append_entries(AppendEntriesRequest {
                term: 2,
                leader_id: "leader-2".into(),
                prev_log_index: 1,
                prev_log_term: 2,
                entries: vec![],
                leader_commit: 1,
                peer_addrs: None,
                learner_addrs: None,
            })
            .await;
        assert!(!response.success);
        assert_eq!(
            node.state.read().await.log.len(),
            1,
            "committed claim removed on prefix mismatch"
        );
    }
}

extern crate self as server;
#[cfg(test)]
#[path = "../../tests/cluster_learner.rs"]
mod learner;
#[cfg(test)]
#[path = "../../tests/cluster_simulation.rs"]
mod simulation;
