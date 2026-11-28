#![cfg(feature = "cluster")]

use scheduler_raft_audit::cluster::*;
use std::{collections::HashMap, time::Duration};
use turmoil::fs::shim::std::fs;

#[test]
fn real_cluster_stops_acknowledging_when_sled_disk_is_full() -> turmoil::Result {
    for full in [false, true] {
        let mut builder = turmoil::Builder::new();
        builder.rng_seed(314).fs().capacity(32_768);
        let mut sim = builder.build();
        sim.client("follower", async move {
            let db = sled::Config::new()
                .path("/raft")
                .flush_every_ms(None)
                .segment_size(4096)
                .cache_capacity(1024 * 1024)
                .open()?;
            let node = ClusterNode::new(
                ClusterConfig {
                    node_id: "follower".into(),
                    peers: vec![],
                    peer_addrs: HashMap::new(),
                    cluster_port: 0,
                    advertise_addr: None,
                    heartbeat_interval: Duration::from_millis(50),
                    election_timeout_min: Duration::from_millis(100),
                    election_timeout_max: Duration::from_millis(200),
                    learner: false,
                },
                db,
            );
            let request = |index, term| AppendEntriesRequest {
                term,
                leader_id: "leader".into(),
                prev_log_index: index - 1,
                prev_log_term: if index == 1 { 0 } else { 1 },
                entries: vec![LogEntry {
                    term,
                    index,
                    key: "receipt".into(),
                    value: format!("value-{index}"),
                    extra: vec![],
                }],
                leader_commit: index,
                peer_addrs: None,
                learner_addrs: None,
            };
            assert!(node.handle_append_entries(request(1, 1)).await.success);
            assert_eq!(node.get("receipt").await?, Some("value-1".into()));
            if full {
                // Real writes consume simulated disk capacity. Never use inject_flush_failure.
                use std::io::Write;
                let mut filler = fs::File::create("/filler")?;
                loop {
                    match filler.write(&[0; 256]) {
                        Ok(_) => {}
                        Err(e) => {
                            assert!(e.to_string().contains("No space left"));
                            break;
                        }
                    }
                }
            }
            let response = node.handle_append_entries(request(2, 2)).await;
            assert_eq!(response.success, !full);
            if full {
                let error = node.db.flush().unwrap_err();
                assert!(error.to_string().contains("No space left"), "{error}");
                let stopped = node.put("later".into(), "value".into()).await.unwrap_err();
                assert!(
                    stopped.contains("cluster stopped or storage failed"),
                    "{stopped}"
                );
                std::mem::forget(node);
            } else {
                assert_eq!(node.get("receipt").await?, Some("value-2".into()));
                drop(node);
            }
            Ok(())
        });
        sim.run()?;
    }
    Ok(())
}
