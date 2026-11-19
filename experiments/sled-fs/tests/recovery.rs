//! Real sled/Turmoil recovery after a disk-full cluster write. This is an
//! isolated storage-boundary fixture, not the general JS scenario runner.
#![cfg(feature = "cluster")]

use scheduler_raft_audit::cluster::*;
use std::{collections::HashMap, sync::{Arc, Mutex, atomic::{AtomicUsize, Ordering}}, time::Duration};
use turmoil::fs::shim::std::fs;

fn config() -> sled::Config {
    sled::Config::new().path("/raft").flush_every_ms(None)
        .segment_size(4096).cache_capacity(1024 * 1024)
}

fn node() -> Result<Arc<ClusterNode>, sled::Error> {
    let db = config().open()?;
    Ok(ClusterNode::new(ClusterConfig {
        node_id: "follower".into(), peers: vec![], peer_addrs: HashMap::new(),
        cluster_port: 0, advertise_addr: None,
        heartbeat_interval: Duration::from_millis(50),
        election_timeout_min: Duration::from_millis(100),
        election_timeout_max: Duration::from_millis(200), learner: false,
    }, db))
}

fn append(index: u64) -> AppendEntriesRequest {
    AppendEntriesRequest {
        term: index, leader_id: "leader".into(),
        prev_log_index: index - 1,
        prev_log_term: if index == 1 { 0 } else { 1 },
        entries: vec![LogEntry {
            term: index, index, key: "receipt".into(),
            value: format!("value-{index}"), extra: vec![],
        }],
        leader_commit: index, peer_addrs: None, learner_addrs: None,
    }
}

fn run(seed: u64) -> turmoil::Result<Vec<String>> {
    let mut builder = turmoil::Builder::new();
    builder.rng_seed(seed).fs().capacity(32_768).sync_probability(0.0);
    let mut sim = builder.build();
    let phase = Arc::new(AtomicUsize::new(0));
    let completed = Arc::new(AtomicUsize::new(0));
    let trace = Arc::new(Mutex::new(Vec::new()));
    let p = phase.clone();
    let c = completed.clone();
    let t = trace.clone();
    sim.host("follower", move || {
        let p = p.clone();
        let c = c.clone();
        let t = t.clone();
        async move {
            let n = node()?;
            if p.load(Ordering::SeqCst) == 0 {
                assert!(n.handle_append_entries(append(1)).await.success);
                assert_eq!(n.get("receipt").await?, Some("value-1".into()));
                n.db.flush()?;
                t.lock().unwrap().push("acked:value-1".into());

                // Fill the actual simulated disk; no ClusterNode fail hook.
                use std::io::Write;
                let mut filler = fs::File::create("/filler")?;
                loop {
                    match filler.write(&[0; 256]) {
                        Ok(_) => {},
                        Err(e) => {
                            assert!(e.to_string().contains("No space left"), "{e}");
                            break;
                        }
                    }
                }
                assert!(!n.handle_append_entries(append(2)).await.success);
                let e = n.db.flush().unwrap_err();
                assert!(e.to_string().contains("No space left"), "{e}");
                t.lock().unwrap().push("rejected:disk-full".into());
                drop(filler);
                // Sim::crash must not run Db/Log destructors that could flush.
                std::mem::forget(n);
            } else {
                assert_eq!(n.get("receipt").await?, Some("value-1".into()));
                t.lock().unwrap().push("recovered:value-1".into());
                // New progress after the disk-full state is discarded on crash.
                assert!(n.handle_append_entries(append(2)).await.success);
                assert_eq!(n.get("receipt").await?, Some("value-2".into()));
                t.lock().unwrap().push("acked:value-2".into());
                drop(n);
            }
            c.fetch_add(1, Ordering::SeqCst);
            std::future::pending::<()>().await;
            Ok(())
        }
    });
    for _ in 0..100 {
        sim.step()?;
        if completed.load(Ordering::SeqCst) == 1 { break; }
    }
    assert_eq!(completed.load(Ordering::SeqCst), 1, "first phase did not finish");
    sim.crash("follower");
    phase.store(1, Ordering::SeqCst);
    sim.bounce("follower");
    for _ in 0..100 {
        sim.step()?;
        if completed.load(Ordering::SeqCst) == 2 { break; }
    }
    assert_eq!(completed.load(Ordering::SeqCst), 2, "recovery did not finish");
    Ok({ let history = trace.lock().unwrap().clone(); history })
}

#[test]
fn disk_full_crash_reopen_preserves_ack_and_allows_fresh_progress() -> turmoil::Result {
    for seed in [7, 42, 314] {
        let first = run(seed)?;
        assert_eq!(first, ["acked:value-1", "rejected:disk-full", "recovered:value-1", "acked:value-2"]);
        assert_eq!(run(seed)?, first, "same seed must replay the observed history");
    }
    Ok(())
}
