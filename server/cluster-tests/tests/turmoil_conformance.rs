//! Figure 2 follower rules through production HTTP handlers and seeded TCP.
//! Persistent reopen uses real sled/OS storage, not simulated power loss.
#[allow(dead_code)]
#[path = "support/peer_sim.rs"]
mod peer_sim;

use peer_sim::{PORT, exchange};
use scheduler_raft_audit::cluster::*;
use std::{cell::RefCell, future::Future, rc::Rc, sync::Arc, time::Duration};
use tokio::{sync::watch, task::JoinSet};
use turmoil::net::TcpListener;

type Incarnation = Rc<RefCell<Option<Arc<ClusterNode>>>>;

fn follower<F, Fut>(seed: u64, test: F)
where
    F: FnOnce(Incarnation, watch::Sender<()>) -> Fut,
    Fut: Future<Output = turmoil::Result> + 'static,
{
    // Storage identity is deliberately independent of the simulation seed.
    let path = std::env::temp_dir().join(format!(
        "raft-conformance-{}-{}",
        std::process::id(),
        rand::random::<u64>()
    ));
    let shared: Incarnation = Rc::new(RefCell::new(None));
    let (restart, receiver) = watch::channel(());
    let mut sim = turmoil::Builder::new()
        .rng_seed(seed)
        .enable_random_order()
        .min_message_latency(Duration::from_millis(1))
        .max_message_latency(Duration::from_millis(3))
        .simulation_duration(Duration::from_secs(5))
        .build();
    let registry = shared.clone();
    let db_path = path.clone();
    sim.host("follower", move || {
        let registry = registry.clone();
        let db_path = db_path.clone();
        let mut receiver = receiver.clone();
        async move {
            loop {
                let node = ClusterNode::new(ClusterConfig::default(), sled::open(&db_path)?);
                let listener = TcpListener::bind(("0.0.0.0", PORT)).await?;
                *registry.borrow_mut() = Some(node.clone());
                let mut connections = JoinSet::new();
                loop {
                    tokio::select! {
                        incoming = listener.accept() => {
                            let (stream, _) = incoming?;
                            let node = node.clone();
                            connections.spawn(async move { serve_cluster_connection(node, stream).await });
                        }
                        done = connections.join_next(), if !connections.is_empty() => {
                            done.expect("connection task").expect("handler panicked").expect("handler I/O");
                        }
                        changed = receiver.changed() => {
                            changed.expect("restart sender must outlive scenario");
                            registry.borrow_mut().take();
                            node.shutdown();
                            connections.abort_all();
                            while connections.join_next().await.is_some() {}
                            drop(listener);
                            drop(node);
                            break;
                        }
                    }
                }
            }
        }
    });
    sim.client("client", test(shared.clone(), restart.clone()));
    let result = sim.run();
    // Keep the sender outside the client: hosts can still be polled between
    // client completion and the simulator observing that completion.
    drop(sim);
    drop(restart);
    shared.borrow_mut().take();
    std::fs::remove_dir_all(&path).expect("remove closed sled store");
    result.unwrap_or_else(|error| panic!("conformance seed={seed}: {error}"));
}

async fn ready(shared: &Incarnation) {
    tokio::time::timeout(Duration::from_secs(1), async {
        while shared.borrow().is_none() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("follower readiness");
}

fn vote(term: u64, candidate: &str, index: u64, log_term: u64) -> RequestVoteRequest {
    RequestVoteRequest {
        term,
        candidate_id: candidate.into(),
        last_log_index: index,
        last_log_term: log_term,
    }
}

async fn request_vote(request: RequestVoteRequest) -> RequestVoteResponse {
    exchange("follower:4000", "/raft/request-vote", &request)
        .await
        .expect("complete vote response")
}

fn entry(index: u64, term: u64, value: &str) -> LogEntry {
    LogEntry {
        index,
        term,
        key: format!("key-{index}"),
        value: value.into(),
        extra: vec![],
    }
}

async fn append(
    term: u64,
    prev: u64,
    prev_term: u64,
    entries: Vec<LogEntry>,
    commit: u64,
) -> AppendEntriesResponse {
    exchange(
        "follower:4000",
        "/raft/append-entries",
        &AppendEntriesRequest {
            term,
            leader_id: "leader".into(),
            prev_log_index: prev,
            prev_log_term: prev_term,
            entries,
            leader_commit: commit,
            peer_addrs: None,
            learner_addrs: None,
        },
    )
    .await
    .expect("complete append response")
}

#[test]
fn vote_is_durable_across_reopen_and_resets_only_in_a_higher_term() {
    for seed in [7, 42, 314] {
        follower(seed, |shared, restart| async move {
            ready(&shared).await;
            assert!(request_vote(vote(3, "a", 0, 0)).await.vote_granted);
            assert!(!request_vote(vote(3, "b", 0, 0)).await.vote_granted);
            let old = Arc::downgrade(shared.borrow().as_ref().unwrap());
            restart.send(()).unwrap();
            tokio::time::timeout(Duration::from_secs(1), async {
                loop {
                    let replaced = shared
                        .borrow()
                        .as_ref()
                        .is_some_and(|node| !std::ptr::eq(Arc::as_ptr(node), old.as_ptr()));
                    if replaced {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            })
            .await
            .expect("new incarnation");
            assert!(old.upgrade().is_none(), "old store must actually be closed");
            let denied = request_vote(vote(3, "b", 0, 0)).await;
            assert_eq!(denied.term, 3);
            assert!(
                !denied.vote_granted,
                "restart must not permit double voting"
            );
            assert!(request_vote(vote(3, "a", 0, 0)).await.vote_granted);
            assert!(request_vote(vote(4, "b", 0, 0)).await.vote_granted);
            let stale = request_vote(vote(3, "a", 0, 0)).await;
            assert_eq!(stale.term, 4);
            assert!(!stale.vote_granted);
            Ok(())
        });
    }
}

#[test]
fn vote_log_freshness_compares_last_term_before_length() {
    // Local log ends at (index=3, term=2). Every row has a fresh store/vote.
    let cases = [
        (0, 0, false),
        (9, 1, false),
        (2, 2, false),
        (3, 2, true),
        (4, 2, true),
        (1, 3, true),
    ];
    for seed in [7, 42, 314] {
        for (index, term, expected) in cases {
            follower(seed, move |shared, _keep_restart| async move {
                ready(&shared).await;
                assert!(
                    append(
                        3,
                        0,
                        0,
                        vec![entry(1, 1, "one"), entry(2, 1, "two"), entry(3, 2, "three")],
                        0
                    )
                    .await
                    .success
                );
                let response = request_vote(vote(4, "candidate", index, term)).await;
                assert_eq!(response.term, 4);
                assert_eq!(
                    response.vote_granted, expected,
                    "candidate index={index} term={term}"
                );
                Ok(())
            });
        }
    }
}

#[test]
fn duplicate_delayed_and_conflicting_appends_preserve_committed_prefix() {
    for seed in [7, 42, 314] {
        follower(seed, |shared, _keep_restart| async move {
            ready(&shared).await;
            let mut first = entry(1, 1, "committed");
            first.extra.push(("receipt".into(), "atomic".into()));
            let initial = vec![first.clone(), entry(2, 1, "uncommitted")];
            assert!(append(1, 0, 0, initial.clone(), 1).await.success);
            assert!(
                append(1, 0, 0, initial, 1).await.success,
                "duplicate delivery"
            );
            // A new leader replaces only the uncommitted suffix.
            assert!(
                append(2, 1, 1, vec![entry(2, 2, "replacement")], 2)
                    .await
                    .success
            );
            assert!(
                append(2, 1, 1, vec![entry(2, 2, "replacement")], 2)
                    .await
                    .success
            );
            let stale = append(1, 0, 0, vec![first.clone(), entry(2, 1, "stale")], 2).await;
            assert_eq!(stale.term, 2);
            assert!(!stale.success);
            assert!(
                !append(3, 1, 9, vec![], 2).await.success,
                "mismatched prev term"
            );
            assert!(
                !append(3, 4, 2, vec![], 4).await.success,
                "missing prev index"
            );
            assert!(
                !append(3, 0, 0, vec![entry(1, 3, "overwrite")], 1)
                    .await
                    .success,
                "committed entry immutable"
            );
            assert!(
                append(3, 1, 1, vec![], 0).await.success,
                "delayed short heartbeat"
            );
            let node = shared.borrow().as_ref().unwrap().clone();
            let state = node.state.read().await;
            assert_eq!(state.log, vec![first, entry(2, 2, "replacement")]);
            assert_eq!(state.commit_index, 2);
            assert_eq!(state.last_applied, 2);
            drop(state);
            assert_eq!(node.get("key-1").await?, Some("committed".into()));
            assert_eq!(node.get("key-2").await?, Some("replacement".into()));
            assert_eq!(node.get("receipt").await?, Some("atomic".into()));
            Ok(())
        });
    }
}
