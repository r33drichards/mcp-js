use chrono::{Duration as Delta, TimeZone, Utc};
use mcp_js_scheduler_core::{Definition, Limits, MissedRuns, Outcome, Overlap, Policy, Status};
use mcp_js_scheduler_runner::{
    Replicated,
    cluster::{ClusterConfig, ClusterNode, Role},
};
use std::{sync::Arc, time::Duration};

// A reported leader can lose its term while restarted peers catch up. Only a
// committed acquisition establishes the owner used by the assertions below.
async fn acquire_owner(
    nodes: &[Arc<ClusterNode>],
    definitions: &[Definition],
    contract: &str,
    now: chrono::DateTime<Utc>,
) -> (usize, Replicated) {
    let mut last_error = String::new();
    let result = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            for (i, node) in nodes.iter().enumerate() {
                if node.status().await.role != Role::Leader {
                    continue;
                }
                match Replicated::acquire(
                    node.clone(),
                    Limits::default(),
                    definitions,
                    contract.into(),
                    now,
                )
                .await
                {
                    Ok(owner) => return (i, owner),
                    Err(error)
                        if error.contains("leadership")
                            || error.contains("leader")
                            || error.contains("raced")
                            || error.contains("term") =>
                    {
                        last_error = error;
                    }
                    Err(error) => panic!("owner acquisition failed: {error}"),
                }
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await;
    result.unwrap_or_else(|_| panic!("committed owner acquisition: {last_error}"))
}
fn configs() -> Vec<ClusterConfig> {
    let sockets: Vec<_> = (0..3)
        .map(|_| std::net::TcpListener::bind("127.0.0.1:0").unwrap())
        .collect();
    let ports: Vec<_> = sockets
        .iter()
        .map(|s| s.local_addr().unwrap().port())
        .collect();
    (0..3)
        .map(|i| {
            let peers: Vec<_> = (0..3)
                .filter(|j| *j != i)
                .map(|j| format!("n{j}@127.0.0.1:{}", ports[j]))
                .collect();
            let (peers, peer_addrs) = ClusterConfig::parse_peers(&peers);
            ClusterConfig {
                node_id: format!("n{i}"),
                peers,
                peer_addrs,
                cluster_port: ports[i],
                advertise_addr: Some(format!("127.0.0.1:{}", ports[i])),
                heartbeat_interval: Duration::from_millis(20),
                election_timeout_min: Duration::from_millis(500),
                election_timeout_max: Duration::from_millis(1000),
                learner: false,
            }
        })
        .collect()
}
fn definition() -> Definition {
    Definition {
        name: Some("job".into()),
        code: "1+1".into(),
        cron: "* * * * *".into(),
        timezone: "UTC".into(),
        policy: Policy {
            missed: MissedRuns::RunOnce,
            overlap: Overlap::Queue,
            ..Default::default()
        },
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fixed_cluster_takeover_restart_and_no_quorum_do_not_replay_consumed_work() {
    let configs = configs();
    let dbs: Vec<_> = (0..3)
        .map(|_| sled::Config::new().temporary(true).open().unwrap())
        .collect();
    let nodes: Vec<_> = configs
        .iter()
        .zip(&dbs)
        .map(|(c, d)| ClusterNode::new_fixed(c.clone(), d.clone()).unwrap())
        .collect();
    for n in &nodes {
        n.start().await;
    }
    let now = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
    let definitions = vec![definition()];
    let (first, mut a) = acquire_owner(&nodes, &definitions, "grant-v1", now).await;
    a.observe(now + Delta::minutes(1)).await.unwrap();
    let consumed = a.claim(now + Delta::minutes(1)).await.unwrap().unwrap();
    nodes[first].shutdown();
    let live: Vec<_> = nodes
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != first)
        .map(|(_, n)| n.clone())
        .collect();
    let (second, mut b) =
        acquire_owner(&live, &definitions, "grant-v1", now + Delta::minutes(3)).await;
    assert!(a.finish(&consumed, Outcome::Succeeded, now).await.is_err());
    let records = b.view().occurrences();
    assert!(
        records
            .iter()
            .any(|o| o.id == consumed.occurrence_id && matches!(o.status, Status::Unknown { .. }))
    );
    b.observe(now + Delta::minutes(3)).await.unwrap();
    assert!(
        b.claim(now + Delta::minutes(3)).await.unwrap().is_none(),
        "unknown holds overlap slot"
    );
    // Remove quorum while keeping the old leader process alive. No committed
    // mutation/claim may be released; cancel its wait and invalidate the owner.
    live[1 - second].shutdown();
    assert!(
        tokio::time::timeout(
            Duration::from_millis(250),
            b.create(definition(), now + Delta::minutes(3))
        )
        .await
        .is_err()
    );
    assert!(b.claim(now).await.is_err());
    for n in &nodes {
        n.shutdown();
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    let restarted: Vec<_> = configs
        .iter()
        .zip(&dbs)
        .map(|(c, d)| ClusterNode::new_fixed(c.clone(), d.clone()).unwrap())
        .collect();
    for n in &restarted {
        n.start().await;
    }
    let (_, mut c) = acquire_owner(
        &restarted,
        &definitions,
        "grant-v1",
        now + Delta::minutes(10),
    )
    .await;
    assert!(
        c.view()
            .occurrences()
            .iter()
            .any(|o| o.id == consumed.occurrence_id && matches!(o.status, Status::Unknown { .. }))
    );
    c.observe(now + Delta::minutes(10)).await.unwrap();
    if let Some(next) = c.claim(now + Delta::minutes(10)).await.unwrap() {
        // The timed-out create may legitimately commit on restart. It must
        // never reopen the consumed occurrence or its blocked schedule.
        assert_ne!(next.schedule_id, consumed.schedule_id);
        assert_ne!(next.occurrence_id, consumed.occurrence_id);
    }
    for n in restarted {
        n.shutdown();
    }
}
#[tokio::test]
async fn generic_api_cannot_rewrite_scheduler_authority() {
    let n = ClusterNode::new_fixed(
        ClusterConfig::default(),
        sled::Config::new().temporary(true).open().unwrap(),
    )
    .unwrap();
    assert!(
        n.put("scheduler/state/v1".into(), "forged".into())
            .await
            .is_err()
    );
    assert!(
        n.cas("scheduler/state/v1".into(), None, "forged".into())
            .await
            .is_err()
    );
    assert!(
        n.put_with(
            "other".into(),
            "value".into(),
            vec![("scheduler/state/v1".into(), "forged".into())]
        )
        .await
        .is_err()
    );
}

#[test]
fn abrupt_process_exit_after_quorum_claim_recovers_unknown() {
    let dir = tempfile::tempdir().unwrap();
    let socket = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = socket.local_addr().unwrap().port();
    drop(socket);
    for phase in ["consume", "recover"] {
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "durable_restart_helper",
                "--ignored",
                "--nocapture",
            ])
            .env("SCHEDULER_TEST_DB", dir.path())
            .env("SCHEDULER_TEST_PORT", port.to_string())
            .env("SCHEDULER_TEST_PHASE", phase)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{phase}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
}
#[tokio::test]
#[ignore = "invoked in isolated subprocess by parent test"]
async fn durable_restart_helper() {
    let path = std::env::var("SCHEDULER_TEST_DB").unwrap();
    let port: u16 = std::env::var("SCHEDULER_TEST_PORT")
        .unwrap()
        .parse()
        .unwrap();
    let node = ClusterNode::new_fixed(
        ClusterConfig {
            cluster_port: port,
            advertise_addr: Some(format!("127.0.0.1:{port}")),
            ..Default::default()
        },
        sled::open(path).unwrap(),
    )
    .unwrap();
    node.start().await;
    let now = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
    let (_, mut owner) = acquire_owner(
        std::slice::from_ref(&node),
        &[definition()],
        "subprocess-grant",
        now,
    )
    .await;
    if std::env::var("SCHEDULER_TEST_PHASE").unwrap() == "consume" {
        owner.observe(now + Delta::minutes(1)).await.unwrap();
        assert!(
            owner
                .claim(now + Delta::minutes(1))
                .await
                .unwrap()
                .is_some()
        );
    } else {
        assert!(
            owner
                .view()
                .occurrences()
                .iter()
                .any(|o| matches!(o.status, Status::Unknown { .. }))
        );
        owner.observe(now + Delta::minutes(3)).await.unwrap();
        assert!(
            owner
                .claim(now + Delta::minutes(3))
                .await
                .unwrap()
                .is_none()
        );
    }
    // Deliberately skip Rust destructors/graceful shutdown after durable ACK.
    std::process::exit(0);
}
