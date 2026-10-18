use super::*;
use crate::{
    RemoteConfig,
    scheduled_protocol::*,
    scheduled_worker::{Config, Launch, Runtime, Worker},
};
use std::sync::atomic::{AtomicUsize, Ordering};
#[derive(Default)]
struct Engine(AtomicUsize);
impl Runtime for Engine {
    fn submit(
        &self,
        _: Payload,
    ) -> Pin<Box<dyn Future<Output = std::result::Result<String, String>> + Send + '_>> {
        Box::pin(async {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok("work-1".into())
        })
    }
    fn status(&self, id: String) -> std::result::Result<Value, String> {
        Ok(serde_json::json!({"execution_id":id,"status":"completed"}))
    }
}
fn remote() -> RemoteConfig {
    RemoteConfig {
        url: "http://127.0.0.1:1".into(),
        namespace: "model".into(),
        worker_id: "worker".into(),
        bearer_env: "PATH".into(),
        expires_at: Utc.with_ymd_and_hms(2099, 1, 1, 0, 0, 0).unwrap(),
        session: "operator".into(),
        timeout_seconds: 30,
        heap_mb: 32,
    }
}
fn worker_config(path: &std::path::Path) -> Config {
    Config {
        namespace: "model".into(),
        worker_id: "worker".into(),
        data_dir: path.to_str().unwrap().into(),
        bearer_env: "PATH".into(),
        admission_bearer_env: "PATH".into(),
        admission_urls: vec!["http://127.0.0.1:1".into()],
    }
}
async fn owner(net: &mut Network) -> Replicated {
    let node = net.nodes[net.leader].clone();
    net.drive(Replicated::acquire(
        node,
        Limits::default(),
        &[definition()],
        "grant".into(),
        now(0),
    ))
    .await
    .unwrap()
}
async fn claimed(net: &mut Network, owner: &mut Replicated) -> Dispatch {
    net.drive(owner.observe(now(1))).await.unwrap();
    net.drive(owner.claim_for_worker(now(1), &remote()))
        .await
        .unwrap()
        .unwrap()
        .1
}
#[tokio::test(start_paused = true)]
async fn takeover_rejects_stale_admission_but_preserves_previously_admitted_launch() {
    for admitted_first in [false, true] {
        let mut net = Network::new().await;
        let mut old = owner(&mut net).await;
        let dispatch = claimed(&mut net, &mut old).await;
        let dir = tempfile::tempdir().unwrap();
        let worker = Worker::new(worker_config(dir.path())).unwrap();
        let request = worker.reserve(dispatch.clone()).unwrap();
        let permit = if admitted_first {
            Some(net.drive(old.admit(request.clone(), now(1))).await.unwrap())
        } else {
            None
        };
        net.elect(1, 2).await;
        let mut new = owner(&mut net).await;
        assert!(new.epoch() > dispatch.epoch);
        assert!(net.drive(new.admit(request.clone(), now(1))).await.is_err());
        let engine = Engine::default();
        if let Some(permit) = permit {
            worker
                .consume(request.clone(), permit.clone())
                .unwrap()
                .run(&engine)
                .await
                .unwrap();
            assert!(worker.consume(request, permit).is_err());
            assert_eq!(engine.0.load(Ordering::SeqCst), 1);
        } else {
            assert_eq!(engine.0.load(Ordering::SeqCst), 0);
        }
        assert!(worker.reserve(dispatch).is_err());
    }
}
#[tokio::test(start_paused = true)]
async fn lost_admission_ack_and_worker_crash_never_recreate_launch_permission() {
    for after_commit in [false, true] {
        let mut net = Network::new().await;
        let mut active = owner(&mut net).await;
        let dispatch = claimed(&mut net, &mut active).await;
        let dir = tempfile::tempdir().unwrap();
        let config = worker_config(dir.path());
        let worker = Worker::new(config.clone()).unwrap();
        let request = worker.reserve(dispatch.clone()).unwrap();
        let mut future = Box::pin(active.admit(request.clone(), now(1)));
        assert!(matches!(poll!(future.as_mut()), Poll::Pending));
        if after_commit {
            net.replicate().await;
        }
        drop(future);
        assert!(active.admit(request.clone(), now(1)).await.is_err());
        drop(worker);
        let restarted = Worker::new(config).unwrap();
        assert!(restarted.reserve(dispatch.clone()).is_err());
        assert!(
            restarted
                .consume(request.clone(), request.permit())
                .is_err()
        );
        net.elect(1, 2).await;
        let mut next = owner(&mut net).await;
        assert!(net.drive(next.admit(request, now(1))).await.is_err());
    }
}
#[tokio::test(start_paused = true)]
async fn admission_and_launch_bindings_duplicates_and_flush_failures() {
    let mut net = Network::new().await;
    let mut active = owner(&mut net).await;
    let dispatch = claimed(&mut net, &mut active).await;
    let dir = tempfile::tempdir().unwrap();
    let worker = Worker::new(worker_config(dir.path())).unwrap();
    let request = worker.reserve(dispatch.clone()).unwrap();
    for field in ["epoch", "authority", "worker_id", "payload_digest"] {
        let mut value = serde_json::to_value(&request).unwrap();
        value["dispatch"][field] = if field == "epoch" {
            Value::from(999)
        } else {
            Value::from("wrong")
        };
        let bad = serde_json::from_value(value).unwrap();
        assert!(net.drive(active.admit(bad, now(1))).await.is_err());
    }
    let permit = net
        .drive(active.admit(request.clone(), now(1)))
        .await
        .unwrap();
    assert!(
        net.drive(active.admit(request.clone(), now(1)))
            .await
            .is_err()
    );
    let mut wrong = permit.clone();
    wrong.worker_incarnation = "other".into();
    assert!(worker.consume(request.clone(), wrong).is_err());
    worker.inject_flush_failure();
    assert!(worker.consume(request, permit).is_err()); // No Launch value escapes.
    let other = tempfile::tempdir().unwrap();
    let duplicate_worker = Worker::new(worker_config(other.path())).unwrap();
    let duplicate = duplicate_worker.reserve(dispatch.clone()).unwrap();
    assert!(net.drive(active.admit(duplicate, now(1))).await.is_err());
    let third = tempfile::tempdir().unwrap();
    let bad_store = Worker::new(worker_config(third.path())).unwrap();
    bad_store.inject_flush_failure();
    assert!(bad_store.reserve(dispatch).is_err());
}
#[tokio::test(start_paused = true)]
async fn abandoned_or_wrong_attempt_cannot_be_admitted() {
    let mut net = Network::new().await;
    let mut active = owner(&mut net).await;
    let dispatch = claimed(&mut net, &mut active).await;
    let dir = tempfile::tempdir().unwrap();
    let worker = Worker::new(worker_config(dir.path())).unwrap();
    let request = worker.reserve(dispatch.clone()).unwrap();
    let invocation = Invocation {
        occurrence_id: dispatch.occurrence_id,
        schedule_id: dispatch.schedule_id,
        due_at: now(1),
        attempt: dispatch.attempt,
        code: dispatch.payload.code.clone(),
    };
    net.drive(active.finish(
        &invocation,
        Outcome::Unknown("lost submission".into()),
        now(1),
    ))
    .await
    .unwrap();
    net.drive(active.acknowledge_stopped(dispatch.occurrence_id))
        .await
        .unwrap();
    assert!(net.drive(active.admit(request, now(1))).await.is_err());
    // Reacquisition in the same Raft term also advances the separate ownership epoch.
    let recovered = owner(&mut net).await;
    assert!(recovered.epoch() > active.epoch());
}

async fn replay_admission(trace: &Value) -> std::result::Result<(), String> {
    let mut net = Network::new().await;
    let mut active = Some(owner(&mut net).await);
    let mut dispatch = None;
    let dirs = [tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap()];
    let configs: Vec<_> = dirs.iter().map(|d| worker_config(d.path())).collect();
    let mut workers: Vec<_> = configs
        .iter()
        .map(|c| Some(Worker::new(c.clone()).unwrap()))
        .collect();
    let mut requests: Vec<Option<Admission>> = vec![None, None];
    let mut permits: Vec<Option<Permit>> = vec![None, None];
    let mut launches: Vec<Option<Launch>> = vec![None, None];
    let mut reserved = [false; 2];
    let mut consumed = [false; 2];
    let engine = Engine::default();
    for state in trace["states"].as_array().unwrap().iter().skip(1) {
        let op = state["event"]["op"].as_str().unwrap();
        let w = state["event"]["worker"].as_u64().unwrap().saturating_sub(1) as usize;
        match op {
            "claim" => dispatch = Some(claimed(&mut net, active.as_mut().unwrap()).await),
            "send" | "duplicate" | "drop" => (), // Explicit delivery choices; transport carries the same bound Dispatch.
            "reserve" => {
                requests[w] = Some(
                    workers[w]
                        .as_ref()
                        .unwrap()
                        .reserve(dispatch.clone().unwrap())
                        .unwrap(),
                );
                reserved[w] = true;
            }
            "reject_duplicate" => assert!(
                workers[w]
                    .as_ref()
                    .unwrap()
                    .reserve(dispatch.clone().unwrap())
                    .is_err()
            ),
            "admit" => {
                let result = net
                    .drive(
                        active
                            .as_mut()
                            .unwrap()
                            .admit(requests[w].clone().unwrap(), now(1)),
                    )
                    .await;
                if let Ok(p) = result {
                    permits[w] = Some(p);
                }
            }
            "lost_ack" | "uncertain" => {
                let mut future = Box::pin(
                    active
                        .as_mut()
                        .unwrap()
                        .admit(requests[w].clone().unwrap(), now(1)),
                );
                assert!(matches!(poll!(future.as_mut()), Poll::Pending));
                if op == "lost_ack" {
                    net.replicate().await;
                } // quorum loss leaves only an uncommitted proposal
                drop(future);
                active = None;
            }
            "consume" => {
                let permit = permits[w]
                    .take()
                    .ok_or("missing production admission permit")?;
                launches[w] = Some(
                    workers[w]
                        .as_ref()
                        .unwrap()
                        .consume(requests[w].clone().unwrap(), permit)
                        .map_err(|e| format!("consume: {e}"))?,
                );
                consumed[w] = true;
            }
            "launch" => launches[w]
                .take()
                .ok_or("missing production launch capability")?
                .run(&engine)
                .await
                .map(|_| ())?,
            "crash" => {
                workers[w] = None;
                permits[w] = None;
                launches[w] = None;
                requests[w] = None;
            }
            "restart" => workers[w] = Some(Worker::new(configs[w].clone()).unwrap()),
            "partition" => net.quorum = false,
            "heal" => net.quorum = true,
            "owner_crash" => {
                active = None;
            }
            "takeover" => {
                net.elect(1, 2).await;
                active = Some(owner(&mut net).await);
            }
            other => return Err(format!("unmapped admission action {other}")),
        }
        let bytes = net.nodes[net.leader]
            .get("scheduler/state/v1")
            .await
            .unwrap()
            .unwrap();
        let envelope: Value = serde_json::from_str(&bytes).unwrap();
        let admitted = envelope["admissions"].as_object().unwrap().len();
        let set = |flags: &[bool]| -> Vec<usize> {
            flags
                .iter()
                .enumerate()
                .filter_map(|(i, v)| v.then_some(i + 1))
                .collect()
        };
        let actual = serde_json::json!({"admitted_count":admitted,"reserved":set(&reserved),"consumed":set(&consumed),
            "grants":set(&permits.iter().map(Option::is_some).collect::<Vec<_>>()),"ready":set(&launches.iter().map(Option::is_some).collect::<Vec<_>>()),
            "launches":engine.0.load(Ordering::SeqCst),"epoch":envelope["epoch"]});
        for field in [
            "admitted_count",
            "reserved",
            "consumed",
            "grants",
            "ready",
            "launches",
            "epoch",
        ] {
            let expected = if field == "admitted_count" {
                Value::from(state["admitted"].as_array().unwrap().len())
            } else {
                state[field].clone()
            };
            if actual[field] != expected {
                return Err(format!(
                    "{} {op} {field}: actual={} model={expected}",
                    state["step"], actual[field]
                ));
            }
        }
    }
    Ok(())
}
#[tokio::test(start_paused = true)]
async fn generated_admission_traces_match_production() {
    let directory = std::env::var_os("SCHEDULER_ADMISSION_TRACES")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../specs/runner/admission-generated")
        });
    let mut paths: Vec<_> = std::fs::read_dir(directory)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.extension().is_some_and(|e| e == "json")
                && !p.to_string_lossy().ends_with(".tlc.json")
                && p.file_name().unwrap() != "summary.json"
        })
        .collect();
    paths.sort();
    assert_eq!(paths.len(), 25);
    let mut actions = std::collections::BTreeSet::new();
    for path in paths {
        let trace: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        for s in trace["states"].as_array().unwrap() {
            actions.insert(s["event"]["op"].as_str().unwrap().to_owned());
        }
        let result = replay_admission(&trace).await;
        if trace["mutation"].is_null() {
            assert!(result.is_ok(), "{}: {result:?}", path.display());
        } else {
            let error = result.expect_err("production accepted unsafe admission mutation");
            assert!(
                error.contains("admitted_count"),
                "unrelated divergence: {error}"
            );
        }
    }
    assert_eq!(actions.len(), 18);
    eprintln!("admission action coverage: {actions:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn actual_http_gate_rejects_duplicate_legacy_and_lost_admission_ack() {
    use axum::{Json, Router, routing::post};
    for lose_ack in [false, true] {
        let node = ClusterNode::new_fixed(
            ClusterConfig {
                cluster_port: 0,
                advertise_addr: Some("127.0.0.1:0".into()),
                ..Default::default()
            },
            sled::Config::new().temporary(true).open().unwrap(),
        )
        .unwrap();
        node.start().await;
        tokio::time::timeout(Duration::from_secs(3), async {
            while node.status().await.role != Role::Leader {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let mut active = Replicated::acquire(
            node.clone(),
            Limits::default(),
            &[definition()],
            "grant".into(),
            now(0),
        )
        .await
        .unwrap();
        active.observe(now(1)).await.unwrap();
        let dispatch = active
            .claim_for_worker(now(1), &remote())
            .await
            .unwrap()
            .unwrap()
            .1;
        let active = Arc::new(AsyncMutex::new(active));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let admission_url = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new().route(
            "/scheduler/v1/admit",
            post(move |Json(request): Json<Admission>| {
                let active = active.clone();
                async move {
                    let result = active.lock().await.admit(request, now(1)).await;
                    match result {
                        Ok(permit) if !lose_ack => (
                            axum::http::StatusCode::CREATED,
                            Json(serde_json::to_value(permit).unwrap()),
                        ),
                        _ => (
                            axum::http::StatusCode::SERVICE_UNAVAILABLE,
                            Json(serde_json::json!({"error":"uncertain"})),
                        ),
                    }
                }
            }),
        );
        let admission_server =
            tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let dir = tempfile::tempdir().unwrap();
        let mut config = worker_config(dir.path());
        config.admission_urls = vec![admission_url];
        let worker = Worker::new(config.clone()).unwrap();
        let engine = Arc::new(Engine::default());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let app = worker.clone().router(engine.clone());
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
                .unwrap()
        });
        let client = reqwest::Client::builder()
            .pool_max_idle_per_host(0)
            .build()
            .unwrap();
        let token = std::env::var("PATH").unwrap();
        let submit = || {
            client
                .post(format!("{url}/api/scheduled/v1/exec"))
                .bearer_auth(&token)
                .json(&dispatch)
                .send()
        };
        let (a, b) = tokio::join!(submit(), submit());
        let accepted = [a.unwrap(), b.unwrap()]
            .iter()
            .filter(|r| r.status() == 202)
            .count();
        assert_eq!(accepted, usize::from(!lose_ack));
        assert_eq!(engine.0.load(Ordering::SeqCst), accepted);
        for path in ["/api/exec", "/mcp", "/sse"] {
            assert_eq!(
                client
                    .post(format!("{url}{path}"))
                    .bearer_auth(&token)
                    .json(&dispatch)
                    .send()
                    .await
                    .unwrap()
                    .status(),
                404
            );
        }
        assert_eq!(
            client
                .post(format!("{url}/api/scheduled/v1/exec"))
                .json(&dispatch)
                .send()
                .await
                .unwrap()
                .status(),
            401
        );
        drop(client);
        stop.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(3), server)
            .await
            .unwrap()
            .unwrap();
        drop(worker);
        let restarted = Worker::new(config).unwrap();
        assert!(restarted.reserve(dispatch).is_err());
        admission_server.abort();
        node.shutdown();
    }
}

#[tokio::test(start_paused = true)]
async fn consumed_permission_dropped_before_launch_is_never_recreated() {
    let mut net = Network::new().await;
    let mut active = owner(&mut net).await;
    let dispatch = claimed(&mut net, &mut active).await;
    let dir = tempfile::tempdir().unwrap();
    let config = worker_config(dir.path());
    let worker = Worker::new(config.clone()).unwrap();
    let request = worker.reserve(dispatch.clone()).unwrap();
    let mut changed = request.clone();
    changed.dispatch.payload.code = "different()".into();
    changed.dispatch.payload_digest = digest(&changed.dispatch.payload);
    assert!(net.drive(active.admit(changed, now(1))).await.is_err());
    let permit = net
        .drive(active.admit(request.clone(), now(1)))
        .await
        .unwrap();
    drop(worker.consume(request.clone(), permit.clone()).unwrap()); // Crash window before engine submission.
    drop(worker);
    let restarted = Worker::new(config).unwrap();
    assert!(restarted.reserve(dispatch).is_err());
    assert!(restarted.consume(request, permit).is_err());
}

#[tokio::test(start_paused = true)]
async fn incompatible_checkpoint_and_foreign_worker_store_are_preserved() {
    let mut net = Network::new().await;
    let _active = owner(&mut net).await;
    let node = net.nodes[0].clone();
    let old = node.get("scheduler/state/v1").await.unwrap().unwrap();
    let mut legacy: Value = serde_json::from_str(&old).unwrap();
    for field in ["version", "epoch", "claims", "admissions"] {
        legacy.as_object_mut().unwrap().remove(field);
    }
    let legacy = legacy.to_string();
    assert!(
        net.drive(node.scheduler_cas(1, "scheduler/state/v1".into(), Some(old), legacy.clone()))
            .await
            .unwrap()
    );
    let result = net
        .drive(Replicated::acquire(
            node.clone(),
            Limits::default(),
            &[definition()],
            "grant".into(),
            now(0),
        ))
        .await;
    assert!(
        result
            .err()
            .unwrap()
            .contains("explicit migration required")
    );
    assert_eq!(node.get("scheduler/state/v1").await.unwrap(), Some(legacy));
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("foreign.sqlite"), b"preserve").unwrap();
    assert!(Worker::new(worker_config(dir.path())).is_err());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    assert_eq!(
        std::fs::read(dir.path().join("foreign.sqlite")).unwrap(),
        b"preserve"
    );
}
