use chrono::{Duration as Delta, Utc};
use http_body_util::{BodyExt, Full};
use hyper::{Response, body::Bytes, server::conn::http1, service::service_fn};
use hyper_util::rt::TokioIo;
use serde_json::{Value, json};
use std::{
    convert::Infallible,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    process::{Child, ChildStdin, ChildStdout, Command},
};

struct Runner {
    child: Child,
    input: ChildStdin,
    output: Lines<BufReader<ChildStdout>>,
}
impl Runner {
    async fn request(&mut self, value: Value) -> Value {
        self.input
            .write_all(format!("{value}\n").as_bytes())
            .await
            .unwrap();
        let line = tokio::time::timeout(Duration::from_secs(8), self.output.next_line())
            .await
            .unwrap()
            .unwrap()
            .expect("runner exited");
        serde_json::from_str(&line).unwrap()
    }
}
async fn owner(runners: &mut [Runner], excluded: Option<usize>) -> usize {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            for (i, runner) in runners.iter_mut().enumerate() {
                if Some(i) == excluded {
                    continue;
                }
                let status = runner.request(json!({"action":"status"})).await;
                if status["cluster"]["role"] == "Leader" && status["owner_acquired"] == true {
                    return i;
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("acquired leader")
}
fn remote(url: &str) -> Value {
    json!({"url":url,"namespace":"binary-test","worker_id":"worker","bearer_env":"PATH","expires_at": Utc::now()+Delta::hours(1),"session":"test-operator","timeout_seconds":30,"heap_mb":32})
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn three_real_binaries_manage_dispatch_and_preserve_unknown_after_leader_death() {
    let directory = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let worker_url = format!("http://{}", listener.local_addr().unwrap());
    let posts = Arc::new(AtomicUsize::new(0));
    let (dispatch_tx, mut dispatch_rx) = tokio::sync::mpsc::unbounded_channel();
    let counter = posts.clone();
    let worker = tokio::spawn(async move {
        loop {
            let (socket, _) = listener.accept().await.unwrap();
            let counter = counter.clone();
            let dispatch_tx = dispatch_tx.clone();
            tokio::spawn(async move {
                let _ = http1::Builder::new()
                    .serve_connection(
                        TokioIo::new(socket),
                        service_fn(move |req: hyper::Request<hyper::body::Incoming>| {
                            let counter = counter.clone();
                            let dispatch_tx = dispatch_tx.clone();
                            async move {
                                assert!(req.headers().contains_key("authorization"));
                                let (status, body) = if req.method() == hyper::Method::POST {
                                    let bytes = req.into_body().collect().await.unwrap().to_bytes();
                                    let dispatch: Value = serde_json::from_slice(&bytes).unwrap();
                                    assert_eq!(dispatch["payload"]["code"], "1+1");
                                    let sender = dispatch["owner"]
                                        .as_str()
                                        .unwrap()
                                        .strip_prefix('r')
                                        .unwrap()
                                        .parse::<usize>()
                                        .unwrap();
                                    dispatch_tx
                                        .send((sender, dispatch["occurrence_id"].clone()))
                                        .unwrap();
                                    counter.fetch_add(1, Ordering::SeqCst);
                                    (202, r#"{"execution_id":"work-1"}"#)
                                } else {
                                    (200, r#"{"execution_id":"work-1","status":"running"}"#)
                                };
                                Ok::<_, Infallible>(
                                    Response::builder()
                                        .status(status)
                                        .body(Full::new(Bytes::from_static(body.as_bytes())))
                                        .unwrap(),
                                )
                            }
                        }),
                    )
                    .await;
            });
        }
    });
    let sockets: Vec<_> = (0..3)
        .map(|_| std::net::TcpListener::bind("127.0.0.1:0").unwrap())
        .collect();
    let ports: Vec<_> = sockets
        .iter()
        .map(|s| s.local_addr().unwrap().port())
        .collect();
    drop(sockets);
    let authority = remote(&worker_url); // Exactly the same immutable contract on all voters.
    let mut runners = Vec::new();
    for i in 0..3 {
        let config = json!({"node_id":format!("r{i}"),"advertise_addr":format!("127.0.0.1:{}",ports[i]),"cluster_port":ports[i],"peers":(0..3).filter(|j| *j!=i).map(|j| format!("r{j}@127.0.0.1:{}",ports[j])).collect::<Vec<_>>(),"admission_addr":"127.0.0.1:0","admission_bearer_env":"PATH", "cluster_bearer_env":"PATH","data_dir":directory.path().join(format!("data-{i}")),"schedules":[],"remote":authority});
        let path = directory.path().join(format!("config-{i}.json"));
        std::fs::write(&path, config.to_string()).unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_mcp-js-scheduler-runner"))
            .arg(path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        runners.push(Runner {
            input: child.stdin.take().unwrap(),
            output: BufReader::new(child.stdout.take().unwrap()).lines(),
            child,
        });
    }
    let first = owner(&mut runners, None).await;
    let created = runners[first].request(json!({"action":"create","definition":{"name":"binary-test","code":"1+1","cron":"* * * * *","timezone":"UTC","policy":{"missed":"RunOnce","overlap":"Queue","retry":{"max_retries":0,"delay_seconds":0}}}})).await;
    let id = created["id"].as_u64().expect("schedule created");
    assert_eq!(
        runners[first].request(json!({"action":"list"})).await["schedules"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    // Real Apalis pulse and real minute cron boundary, bounded by one minute + grace.
    // Leadership may change while waiting for the cron boundary. Kill the
    // owner recorded in the actual dispatched payload, not the startup leader.
    let (killed, occurrence_id) = tokio::time::timeout(Duration::from_secs(75), dispatch_rx.recv())
        .await
        .expect("scheduled HTTP submission")
        .unwrap();
    runners[killed].child.kill().await.unwrap();
    runners[killed].child.wait().await.unwrap();
    let second = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            for (i, runner) in runners.iter_mut().enumerate() {
                if i == killed {
                    continue;
                }
                let status = runner.request(json!({"action":"status"})).await;
                if status["cluster"]["role"] != "Leader" || status["owner_acquired"] != true {
                    continue;
                }
                let view = runner.request(json!({"action":"list"})).await;
                if view["occurrences"].as_array().is_some_and(|occurrences| {
                    occurrences
                        .iter()
                        .any(|o| o["id"] == occurrence_id && o["status"].get("Unknown").is_some())
                }) {
                    return i;
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("takeover must recover the exact dispatched occurrence as Unknown");
    assert_eq!(
        runners[second]
            .request(json!({"action":"disable","id":id}))
            .await["ok"],
        true
    );
    assert!(runners[second].request(json!({"action":"acknowledge_stopped","occurrence_id":1,"dispatcher_and_work_quiesced":false})).await.get("error").is_some());
    assert_eq!(
        runners[second]
            .request(json!({"action":"delete","id":id}))
            .await["ok"],
        true
    );
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(
        posts.load(Ordering::SeqCst),
        1,
        "takeover replayed the consumed attempt"
    );
    for (i, r) in runners.iter_mut().enumerate() {
        if i != killed {
            r.child.kill().await.unwrap();
            r.child.wait().await.unwrap();
        }
    }
    worker.abort();
}
#[test]
fn existing_unsealed_or_foreign_store_is_rejected_without_reset() {
    for kind in ["sled", "sqlite"] {
        let directory = tempfile::tempdir().unwrap();
        let data = directory.path().join("old-data");
        std::fs::create_dir(&data).unwrap();
        if kind == "sled" {
            let db = sled::open(&data).unwrap();
            db.open_tree("scheduler_state")
                .unwrap()
                .insert("state", "preserved")
                .unwrap();
            db.flush().unwrap();
        } else {
            std::fs::write(data.join("state.sqlite3"), "preserved").unwrap();
        }
        let config = json!({"node_id":"r","advertise_addr":"127.0.0.1:4401","cluster_port":4401,"peers":[],"admission_addr":"127.0.0.1:0","admission_bearer_env":"PATH", "cluster_bearer_env":"PATH","data_dir":data,"remote":remote("http://127.0.0.1:1")});
        let path = directory.path().join("config.json");
        std::fs::write(&path, config.to_string()).unwrap();
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_mcp-js-scheduler-runner"))
            .arg(path)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("migration required"));
        if kind == "sled" {
            let db = sled::open(&data).unwrap();
            assert_eq!(
                db.open_tree("scheduler_state")
                    .unwrap()
                    .get("state")
                    .unwrap()
                    .unwrap()
                    .as_ref(),
                b"preserved"
            );
            assert!(db.get("raft_fixed_voters").unwrap().is_none());
        } else {
            assert_eq!(
                std::fs::read_to_string(data.join("state.sqlite3")).unwrap(),
                "preserved"
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn graceful_shutdown_keeps_admission_callbacks_alive_until_dispatch_drains() {
    use axum::{
        Json, Router,
        routing::{get, post},
    };
    use mcp_js_scheduler_runner::scheduled_protocol::{Admission, Dispatch};
    let dir = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let worker_url = format!("http://{}", listener.local_addr().unwrap());
    let release = Arc::new(tokio::sync::Notify::new());
    let (dispatch_tx, mut dispatch_rx) = tokio::sync::mpsc::channel(1);
    let unblock = release.clone();
    let app = Router::new()
        .route(
            "/api/scheduled/v1/exec",
            post(move |Json(dispatch): Json<Dispatch>| {
                let tx = dispatch_tx.clone();
                let unblock = unblock.clone();
                async move {
                    tx.send(dispatch).await.unwrap();
                    unblock.notified().await;
                    (
                        axum::http::StatusCode::ACCEPTED,
                        Json(json!({"execution_id":"draining-work"})),
                    )
                }
            }),
        )
        .route(
            "/api/scheduled/v1/executions/{id}",
            get(|| async { Json(json!({"execution_id":"draining-work","status":"completed"})) }),
        );
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let sockets: Vec<_> = (0..2)
        .map(|_| std::net::TcpListener::bind("127.0.0.1:0").unwrap())
        .collect();
    let ports: Vec<_> = sockets
        .iter()
        .map(|s| s.local_addr().unwrap().port())
        .collect();
    drop(sockets);
    let config = json!({"node_id":"drain-runner","advertise_addr":format!("127.0.0.1:{}",ports[0]),"cluster_port":ports[0],
        "peers":[],"data_dir":dir.path().join("state"),"admission_addr":format!("127.0.0.1:{}",ports[1]),
        "admission_bearer_env":"PATH","cluster_bearer_env":"PATH","schedules":[],"remote":remote(&worker_url)});
    let config_path = dir.path().join("runner.json");
    std::fs::write(&config_path, serde_json::to_vec(&config).unwrap()).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_mcp-js-scheduler-runner"))
        .arg(&config_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stderr = BufReader::new(child.stderr.take().unwrap()).lines();
    let runner = Runner {
        input: child.stdin.take().unwrap(),
        output: BufReader::new(child.stdout.take().unwrap()).lines(),
        child,
    };
    let mut runners = vec![runner];
    owner(&mut runners, None).await;
    let result = runners[0]
        .request(json!({"action":"create","definition":{
            "name":"drain-test","code":"1+1","cron":"* * * * *","timezone":"UTC",
            "policy":{"missed":"Skip","overlap":"Skip","retry":{"max_retries":0,"delay_seconds":0}}
        }}))
        .await;
    assert!(result.get("id").is_some(), "{result}");
    let dispatch = tokio::time::timeout(Duration::from_secs(75), dispatch_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(
        Command::new("kill")
            .args(["-INT", &runners[0].child.id().unwrap().to_string()])
            .status()
            .await
            .unwrap()
            .success()
    );
    tokio::time::timeout(Duration::from_secs(3), async {
        while let Some(line) = stderr.next_line().await.unwrap() {
            if line.contains("draining accepted dispatches") {
                return;
            }
        }
        panic!("runner exited without beginning graceful drain");
    })
    .await
    .unwrap();
    let response = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{}/scheduler/v1/admit", ports[1]))
        .bearer_auth(std::env::var("PATH").unwrap())
        .json(&Admission {
            dispatch,
            worker_incarnation: "draining-worker".into(),
        })
        .timeout(Duration::from_secs(3))
        .send()
        .await
        .expect("admission must remain responsive during drain");
    assert_eq!(response.status(), 201);
    release.notify_one();
    assert!(
        tokio::time::timeout(Duration::from_secs(5), runners[0].child.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    server.abort();
}
