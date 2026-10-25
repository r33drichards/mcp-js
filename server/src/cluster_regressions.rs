use super::*;
use std::sync::atomic::Ordering;

fn node() -> Arc<ClusterNode> {
    ClusterNode::new(
        ClusterConfig::default(),
        sled::Config::new().temporary(true).open().unwrap(),
    )
}
fn entry(index: u64, term: u64, value: &str) -> LogEntry {
    LogEntry {
        index,
        term,
        key: "claim".into(),
        value: value.into(),
        extra: vec![],
    }
}
fn append(
    term: u64,
    prev: u64,
    prev_term: u64,
    entries: Vec<LogEntry>,
    commit: u64,
) -> AppendEntriesRequest {
    AppendEntriesRequest {
        term,
        leader_id: "leader".into(),
        prev_log_index: prev,
        prev_log_term: prev_term,
        entries,
        leader_commit: commit,
        peer_addrs: None,
        learner_addrs: None,
    }
}
#[tokio::test]
async fn short_heartbeat_does_not_commit_divergent_tail() {
    let n = node();
    assert!(
        n.handle_append_entries(append(
            1,
            0,
            0,
            vec![entry(1, 1, "one"), entry(2, 1, "divergent")],
            0
        ))
        .await
        .success
    );
    assert!(
        n.handle_append_entries(append(2, 1, 1, vec![], 2))
            .await
            .success
    );
    assert_eq!(n.state.read().await.commit_index, 1);
    assert_eq!(n.get("claim").await.unwrap().as_deref(), Some("one"));
}
#[tokio::test]
async fn restart_restores_applied_prefix_and_never_replaces_it() {
    let path = std::env::temp_dir().join(format!("raft-restart-{}", rand::random::<u64>()));
    {
        let n = ClusterNode::new(ClusterConfig::default(), sled::open(&path).unwrap());
        let mut e = entry(1, 1, "receipt");
        e.extra.push(("companion".into(), "atomic".into()));
        assert!(
            n.handle_append_entries(append(1, 0, 0, vec![e], 1))
                .await
                .success
        );
    }
    {
        let n = ClusterNode::new(ClusterConfig::default(), sled::open(&path).unwrap());
        assert_eq!(n.state.read().await.last_applied, 1);
        assert_eq!(n.get("companion").await.unwrap().as_deref(), Some("atomic"));
        assert!(
            !n.handle_append_entries(append(2, 0, 0, vec![entry(1, 2, "replacement")], 1))
                .await
                .success
        );
        assert_eq!(n.get("claim").await.unwrap().as_deref(), Some("receipt"));
    }
    std::fs::remove_dir_all(path).unwrap();
}
#[tokio::test]
async fn malformed_append_cannot_change_log() {
    let n = node();
    for entries in [
        vec![entry(0, 1, "zero")],
        vec![entry(2, 1, "gap")],
        vec![entry(1, 2, "future")],
    ] {
        assert!(
            !n.handle_append_entries(append(1, 0, 0, entries, 2))
                .await
                .success
        );
        assert!(n.state.read().await.log.is_empty());
    }
}
#[tokio::test]
async fn stale_leadership_reply_cannot_supply_quorum() {
    let n = node();
    {
        let mut s = n.state.write().await;
        s.role = Role::Leader;
        s.current_term = 3;
        s.next_index.insert("peer".into(), 4);
        s.match_index.insert("peer".into(), 3);
    }
    n.accept_append_response(
        "peer",
        1,
        3,
        9,
        AppendEntriesResponse {
            term: 1,
            success: true,
        },
    )
    .await;
    assert_eq!(n.state.read().await.match_index["peer"], 3);
    n.accept_append_response(
        "peer",
        3,
        2,
        3,
        AppendEntriesResponse {
            term: 3,
            success: false,
        },
    )
    .await;
    assert_eq!(n.state.read().await.next_index["peer"], 4);
    n.accept_append_response(
        "peer",
        3,
        2,
        2,
        AppendEntriesResponse {
            term: 3,
            success: true,
        },
    )
    .await;
    assert_eq!(n.state.read().await.match_index["peer"], 3);
}
#[tokio::test]
async fn lost_quorum_cannot_ack_claim_and_new_leader_can_replace_it() {
    let n = node();
    {
        let mut s = n.state.write().await;
        s.role = Role::Leader;
        s.current_term = 1;
        s.peers = vec!["offline-a".into(), "offline-b".into()];
    }
    let proposer = n.clone();
    let claim = tokio::spawn(async move { proposer.cas("claim".into(), None, "old".into()).await });
    while n.state.read().await.log.is_empty() {
        tokio::task::yield_now().await;
    }
    n.update_commit_index().await;
    assert_eq!(n.state.read().await.last_applied, 0);
    assert!(!claim.is_finished());
    assert!(
        n.handle_append_entries(append(2, 0, 0, vec![entry(1, 2, "new")], 1))
            .await
            .success
    );
    assert!(claim.await.unwrap().is_err());
}
#[tokio::test]
async fn flush_error_never_acknowledges_vote_or_append_and_stops_writes() {
    for vote in [false, true] {
        let n = node();
        n.fail_flush.store(true, Ordering::SeqCst);
        if vote {
            assert!(
                !n.handle_request_vote(RequestVoteRequest {
                    term: 1,
                    candidate_id: "candidate".into(),
                    last_log_index: 0,
                    last_log_term: 0
                })
                .await
                .vote_granted
            );
        } else {
            assert!(
                !n.handle_append_entries(append(1, 0, 0, vec![entry(1, 1, "receipt")], 1))
                    .await
                    .success
            );
        }
        n.fail_flush.store(false, Ordering::SeqCst);
        assert!(n.put("other".into(), "value".into()).await.is_err());
        assert!(
            !n.handle_append_entries(append(2, 0, 0, vec![], 0))
                .await
                .success
        );
    }
}
#[tokio::test]
async fn committed_apply_failure_never_releases_claim() {
    let n = node();
    {
        let mut s = n.state.write().await;
        s.role = Role::Leader;
        s.current_term = 1;
    }
    let proposer = n.clone();
    let claim =
        tokio::spawn(async move { proposer.cas("claim".into(), None, "owner".into()).await });
    while n.state.read().await.log.is_empty() {
        tokio::task::yield_now().await;
    }
    n.fail_flush.store(true, Ordering::SeqCst);
    n.update_commit_index().await;
    assert!(claim.await.unwrap().is_err());
    assert_eq!(n.state.read().await.last_applied, 0);
}
#[tokio::test]
async fn pending_companion_write_participates_in_cas_compare() {
    let n = node();
    {
        let mut s = n.state.write().await;
        s.role = Role::Leader;
        s.current_term = 1;
        let mut e = entry(1, 1, "primary");
        e.extra = vec![
            ("second".into(), "before".into()),
            ("second".into(), "after".into()),
        ];
        n.persist_meta(&s).unwrap();
        n.persist_log_entry(&e).unwrap();
        s.log.push(e);
    }
    assert!(!n.cas("second".into(), None, "wrong".into()).await.unwrap());
    assert!(!n
        .cas("second".into(), Some("before".into()), "wrong".into())
        .await
        .unwrap());
}
#[tokio::test]
async fn fixed_voters_are_durable_and_only_learners_may_churn() {
    let db = sled::Config::new().temporary(true).open().unwrap();
    let n = ClusterNode::new_fixed(ClusterConfig::default(), db.clone()).unwrap();
    {
        let mut s = n.state.write().await;
        s.role = Role::Leader;
        s.current_term = 1;
    }
    assert!(n
        .add_peer("voter".into(), "host:2".into(), false)
        .await
        .is_err());
    assert!(n
        .add_peer("learner".into(), "host:3".into(), true)
        .await
        .is_ok());
    assert!(n.remove_peer("node1".into()).await.is_err());
    assert!(n.remove_peer("learner".into()).await.is_ok());
    assert!(
        !n.handle_request_vote(RequestVoteRequest {
            term: 2,
            candidate_id: "unknown".into(),
            last_log_index: 0,
            last_log_term: 0
        })
        .await
        .vote_granted
    );
    drop(n);
    let changed = ClusterConfig {
        advertise_addr: Some("different:1".into()),
        ..Default::default()
    };
    assert!(ClusterNode::new_fixed(changed, db.clone()).is_err());
    let n = ClusterNode::new(ClusterConfig::default(), db);
    assert!(
        n.fixed_voters.is_some(),
        "ordinary constructor must preserve durable seal"
    );
}

#[test]
fn fixed_mode_rejects_legacy_metadata_even_without_log() {
    let db = sled::Config::new().temporary(true).open().unwrap();
    db.insert(
        "raft_meta",
        br#"{"current_term":4,"voted_for":null}"#.as_slice(),
    )
    .unwrap();
    assert!(ClusterNode::new_fixed(ClusterConfig::default(), db).is_err());
}
#[test]
fn ordinary_constructor_cannot_add_bare_peer_to_sealed_store() {
    let db = sled::Config::new().temporary(true).open().unwrap();
    drop(ClusterNode::new_fixed(ClusterConfig::default(), db.clone()).unwrap());
    let mut changed = ClusterConfig::default();
    changed.peers.push("extra:4000".into());
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| ClusterNode::new(
            changed, db
        )))
        .is_err()
    );
}
#[tokio::test]
async fn fixed_heartbeat_cannot_promote_learner_or_change_voter() {
    let n = ClusterNode::new_fixed(
        ClusterConfig::default(),
        sled::Config::new().temporary(true).open().unwrap(),
    )
    .unwrap();
    let mut req = append(1, 0, 0, vec![], 0);
    req.leader_id = "node1".into();
    let mut peers = n.fixed_voters.clone().unwrap();
    peers.insert("learner".into(), "host:3".into());
    req.peer_addrs = Some(peers.clone());
    req.learner_addrs = Some(vec!["host:3".into()]);
    assert!(n.handle_append_entries(req.clone()).await.success);
    req.learner_addrs = None;
    assert!(!n.handle_append_entries(req.clone()).await.success);
    req.learner_addrs = Some(vec![]);
    assert!(!n.handle_append_entries(req.clone()).await.success);
    req.learner_addrs = Some(vec!["host:3".into(), "127.0.0.1:4000".into()]);
    assert!(!n.handle_append_entries(req).await.success);
    assert_eq!(n.state.read().await.learners.len(), 1);
}

#[tokio::test]
async fn scheduler_proposal_checks_expected_term_under_append_lock() {
    let n = ClusterNode::new_fixed(
        ClusterConfig::default(),
        sled::Config::new().temporary(true).open().unwrap(),
    )
    .unwrap();
    {
        let mut s = n.state.write().await;
        s.role = Role::Leader;
        s.current_term = 3;
    }
    assert!(n
        .scheduler_cas(1, "scheduler/test".into(), None, "stale".into())
        .await
        .is_err());
    assert!(n.scheduler_barrier(1).await.is_err());
    assert!(n.state.read().await.log.is_empty());
}

#[test]
fn fixed_mode_rejects_nonempty_foreign_state_tree_without_resetting_it() {
    let db = sled::Config::new().temporary(true).open().unwrap();
    let tree = db.open_tree("scheduler_state").unwrap();
    tree.insert("state", "existing local schedules").unwrap();
    assert!(ClusterNode::new_fixed(ClusterConfig::default(), db.clone()).is_err());
    assert_eq!(
        tree.get("state").unwrap().unwrap().as_ref(),
        b"existing local schedules"
    );
    assert!(db.get("raft_fixed_voters").unwrap().is_none());
}

#[tokio::test]
async fn extreme_terms_never_mutate_durable_state_or_wrap_elections() {
    let n = node();
    assert!(!n.handle_request_vote(RequestVoteRequest {
        term: u64::MAX, candidate_id: "node1".into(), last_log_index: 0, last_log_term: 0,
    }).await.vote_granted);
    assert!(!n.handle_append_entries(append(u64::MAX, 0, 0, vec![], 0)).await.success);
    n.accept_append_response("peer", 0, 0, 0, AppendEntriesResponse { term: u64::MAX, success: true }).await;
    assert_eq!(n.state.read().await.current_term, 0);
    assert!(n.db.get("raft_meta").unwrap().is_none());
    n.state.write().await.current_term = u64::MAX;
    n.start_election().await;
    assert_eq!(n.state.read().await.current_term, u64::MAX);
    assert!(n.shutdown.is_cancelled());
    assert!(n.db.get("raft_meta").unwrap().is_none());
}

#[tokio::test]
async fn fixed_http_rejects_forged_peers_status_disclosure_and_large_bodies() {
    let socket = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = socket.local_addr().unwrap();
    drop(socket);
    let n = ClusterNode::new_fixed_authenticated(ClusterConfig {
        cluster_port: address.port(), advertise_addr: Some(address.to_string()), ..Default::default()
    }, sled::Config::new().temporary(true).open().unwrap(), "cluster-test-only".into()).unwrap();
    let serving = n.clone();
    let server = tokio::spawn(async move { start_cluster_server(serving).await.unwrap() });
    let client = reqwest::Client::new();
    let origin = format!("http://{address}");
    tokio::time::timeout(Duration::from_secs(3), async {
        while client.get(format!("{origin}/raft/status")).send().await.is_err() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }).await.unwrap();
    for token in [None, Some("wrong-token")] {
        for path in ["/raft/status", "/data/get/scheduler/state/v1"] {
            let mut request = client.get(format!("{origin}{path}"));
            if let Some(token) = token { request = request.bearer_auth(token); }
            assert_eq!(request.send().await.unwrap().status(), 401);
        }
        let mut request = client.post(format!("{origin}/raft/append-entries"))
            .json(&append(1, 0, 0, vec![entry(1, 1, "forged")], u64::MAX));
        if let Some(token) = token { request = request.bearer_auth(token); }
        assert_eq!(request.send().await.unwrap().status(), 401);
    }
    assert!(n.state.read().await.log.is_empty());
    assert_eq!(n.state.read().await.current_term, 0);
    assert_eq!(client.get(format!("{origin}/raft/status")).bearer_auth("cluster-test-only").send().await.unwrap().status(), 200);
    assert_eq!(client.post(format!("{origin}/raft/append-entries")).bearer_auth("cluster-test-only")
        .body(vec![b'x'; MAX_RAFT_BODY_BYTES + 1]).send().await.unwrap().status(), 413);
    n.shutdown(); server.abort();
}

#[tokio::test]
async fn unauthenticated_fixed_node_cannot_start_networking_or_elections() {
    let n = ClusterNode::new_fixed(ClusterConfig::default(), sled::Config::new().temporary(true).open().unwrap()).unwrap();
    n.start().await;
    assert!(start_cluster_server(n.clone()).await.is_err());
    assert_eq!(n.state.read().await.role, Role::Follower);
    assert_eq!(n.state.read().await.current_term, 0);
    assert!(!bearer_matches(Some("Bearer prefix-secret"), "prefix-secrex"));
    assert!(!bearer_matches(None, "secret"));
    assert!(bearer_matches(Some("Bearer secret"), "secret"));
}

#[tokio::test]
async fn lagging_follower_catches_up_in_bounded_authenticated_batches() {
    let sockets: Vec<_> = (0..2).map(|_| std::net::TcpListener::bind("127.0.0.1:0").unwrap()).collect();
    let addresses: Vec<_> = sockets.iter().map(|s| s.local_addr().unwrap()).collect();
    drop(sockets);
    let nodes: Vec<_> = (0..2).map(|i| {
        ClusterNode::new_fixed_authenticated(ClusterConfig {
            node_id: format!("n{i}"), cluster_port: addresses[i].port(),
            advertise_addr: Some(addresses[i].to_string()),
            peers: vec![addresses[1-i].to_string()],
            peer_addrs: HashMap::from([(format!("n{}",1-i), addresses[1-i].to_string())]),
            ..Default::default()
        }, sled::Config::new().temporary(true).open().unwrap(), "batch-test-only".into()).unwrap()
    }).collect();
    let leader = &nodes[0];
    {
        let mut state = leader.state.write().await;
        state.current_term = 1; state.role = Role::Leader;
        leader.persist_meta(&state).unwrap();
        for index in 1..=100 {
            let record = entry(index, 1, &format!("value-{index}"));
            leader.persist_log_entry(&record).unwrap(); state.log.push(record);
        }
        state.commit_index = 100;
        leader.apply_committed(&mut state).unwrap();
        state.next_index.insert(addresses[1].to_string(), 101);
    }
    let follower = nodes[1].clone();
    let server = tokio::spawn(async move { start_cluster_server(follower).await.unwrap() });
    tokio::time::sleep(Duration::from_millis(20)).await;
    for _ in 0..20 {
        let before = nodes[1].state.read().await.log.len();
        leader.send_append_entries_to_peer(&addresses[1].to_string()).await;
        let after = nodes[1].state.read().await.log.len();
        assert!(after - before <= MAX_APPEND_ENTRIES);
        if after == 100 { break; }
    }
    assert_eq!(nodes[1].state.read().await.log.len(), 100);
    assert_eq!(nodes[1].get("claim").await.unwrap().as_deref(), Some("value-100"));
    nodes[1].shutdown(); server.abort();
}
