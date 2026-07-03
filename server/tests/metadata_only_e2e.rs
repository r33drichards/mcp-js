/// End-to-end tests for metadata-only mode (`--metadata-only`).
///
/// A metadata-only node serves Raft replication of session metadata (session
/// log, heap tags, fs labels) and nothing else: no V8 engine, no MCP
/// transport, no REST API, no policies. Each test spawns the real server
/// binary and observes the Raft HTTP surface on `--cluster-port`, which in
/// this mode is the node's entire surface.

use std::process::Stdio;
use std::time::Duration;

use tokio::net::TcpListener;
use tokio::process::{Child, Command};

/// Reserve a free localhost port (released before the server binds it).
async fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind to an ephemeral port");
    listener.local_addr().unwrap().port()
}

fn temp_dir(tag: &str) -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix(&format!("mcp-metadata-e2e-{tag}-"))
        .tempdir()
        .expect("create temp dir")
}

fn spawn_server(args: &[&str]) -> Child {
    Command::new(env!("CARGO_BIN_EXE_server"))
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn server binary")
}

/// Poll `/raft/status` on a cluster port until the node reports the expected
/// role, returning the final status JSON.
async fn wait_for_role(port: u16, role: &str) -> serde_json::Value {
    let url = format!("http://127.0.0.1:{port}/raft/status");
    let client = reqwest::Client::new();
    for _ in 0..150 {
        if let Ok(response) = client.get(&url).send().await {
            if let Ok(status) = response.json::<serde_json::Value>().await {
                if status["role"] == role {
                    return status;
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("node on cluster port {port} never reached role {role}");
}

#[tokio::test]
async fn metadata_only_node_leads_and_replicates_data() {
    let dir = temp_dir("leader");
    let port = free_port().await;
    let port_str = port.to_string();
    let advertise = format!("127.0.0.1:{port}");
    let db_path = dir.path().join("sessions").to_string_lossy().to_string();

    let _node = spawn_server(&[
        "--metadata-only",
        "--cluster-port", &port_str,
        "--node-id", "meta1",
        "--advertise-addr", &advertise,
        "--session-db-path", &db_path,
    ]);

    // A single voter elects itself leader.
    let status = wait_for_role(port, "Leader").await;
    assert_eq!(status["node_id"], "meta1");

    // The replicated data API works: a put commits (single-voter majority)
    // and reads back.
    let client = reqwest::Client::new();
    let put = client
        .post(format!("http://127.0.0.1:{port}/data/put"))
        .json(&serde_json::json!({"key": "session:abc", "value": "heap-123"}))
        .send()
        .await
        .expect("put request");
    assert!(put.status().is_success(), "put failed: {:?}", put.text().await);

    let get: serde_json::Value = client
        .get(format!("http://127.0.0.1:{port}/data/get/session:abc"))
        .send()
        .await
        .expect("get request")
        .json()
        .await
        .expect("get response json");
    assert_eq!(get["value"], "heap-123");
}

#[tokio::test]
async fn full_node_joins_metadata_leader_as_learner() {
    let meta_dir = temp_dir("meta");
    let worker_dir = temp_dir("worker");

    let meta_port = free_port().await;
    let worker_cluster_port = free_port().await;
    let worker_http_port = free_port().await;

    let meta_port_str = meta_port.to_string();
    let meta_advertise = format!("127.0.0.1:{meta_port}");
    let meta_db = meta_dir.path().join("sessions").to_string_lossy().to_string();

    // The metadata-only node is the sole voter: the stable anchor for the
    // cluster's session/heap metadata.
    let _meta = spawn_server(&[
        "--metadata-only",
        "--cluster-port", &meta_port_str,
        "--node-id", "meta1",
        "--advertise-addr", &meta_advertise,
        "--session-db-path", &meta_db,
    ]);
    wait_for_role(meta_port, "Leader").await;

    // A full (JS-executing) node joins as a non-voting learner, so its churn
    // never affects the metadata quorum.
    let worker_cluster_str = worker_cluster_port.to_string();
    let worker_http_str = worker_http_port.to_string();
    let worker_advertise = format!("127.0.0.1:{worker_cluster_port}");
    let worker_db = worker_dir.path().join("sessions").to_string_lossy().to_string();
    let heap_dir = worker_dir.path().join("heaps").to_string_lossy().to_string();

    let _worker = spawn_server(&[
        "--http-port", &worker_http_str,
        "--heap-store", "dir",
        "--heap-dir", &heap_dir,
        "--cluster-port", &worker_cluster_str,
        "--node-id", "worker1",
        "--advertise-addr", &worker_advertise,
        "--join", &meta_advertise,
        "--join-as-learner",
        "--session-db-path", &worker_db,
    ]);
    wait_for_role(worker_cluster_port, "Follower").await;

    // The leader classifies the worker as a learner.
    let client = reqwest::Client::new();
    let mut saw_learner = false;
    for _ in 0..100 {
        let status: serde_json::Value = client
            .get(format!("http://127.0.0.1:{meta_port}/raft/status"))
            .send()
            .await
            .expect("leader status")
            .json()
            .await
            .expect("leader status json");
        let learners = status["learners"].as_array().cloned().unwrap_or_default();
        if learners.iter().any(|l| l == &serde_json::json!(worker_advertise.clone())) {
            saw_learner = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(saw_learner, "leader never classified worker1 as a learner");

    // A write submitted to the worker forwards to the metadata leader,
    // commits there, and replicates back to the worker. Retry until the
    // worker has learned the leader's address from a heartbeat.
    let mut forwarded = false;
    for _ in 0..100 {
        let put = client
            .post(format!("http://127.0.0.1:{worker_cluster_port}/data/put"))
            .json(&serde_json::json!({"key": "session:xyz", "value": "heap-456"}))
            .send()
            .await
            .expect("forwarded put");
        if put.status().is_success() {
            forwarded = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(forwarded, "put via the learner never reached the metadata leader");

    let leader_get: serde_json::Value = client
        .get(format!("http://127.0.0.1:{meta_port}/data/get/session:xyz"))
        .send()
        .await
        .expect("leader get")
        .json()
        .await
        .expect("leader get json");
    assert_eq!(leader_get["value"], "heap-456");

    let mut replicated = false;
    for _ in 0..100 {
        let worker_get: serde_json::Value = client
            .get(format!("http://127.0.0.1:{worker_cluster_port}/data/get/session:xyz"))
            .send()
            .await
            .expect("worker get")
            .json()
            .await
            .expect("worker get json");
        if worker_get["value"] == "heap-456" {
            replicated = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(replicated, "committed write never replicated to the learner");
}

/// Run the binary and capture its exit status and stderr (for flag-validation
/// failures, which exit before any server starts).
fn run_expecting_failure(args: &[&str]) -> (bool, String) {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_server"))
        .args(args)
        .stdin(Stdio::null())
        .output()
        .expect("run server binary");
    (output.status.success(), String::from_utf8_lossy(&output.stderr).to_string())
}

#[test]
fn metadata_only_requires_cluster_port() {
    let (success, stderr) = run_expecting_failure(&["--metadata-only"]);
    assert!(!success, "--metadata-only without --cluster-port should fail");
    assert!(
        stderr.contains("--cluster-port"),
        "error should point at the missing --cluster-port, got: {stderr}"
    );
}

#[test]
fn metadata_only_conflicts_with_mcp_transports_and_js_config() {
    for conflicting in [
        vec!["--http-port", "39901"],
        vec!["--sse-port", "39902"],
        vec!["--policies-json", "{}"],
        vec!["--allow-run-js-file"],
    ] {
        let mut args = vec!["--metadata-only", "--cluster-port", "39900"];
        args.extend(conflicting.iter().copied());
        let (success, stderr) = run_expecting_failure(&args);
        assert!(
            !success,
            "--metadata-only with {conflicting:?} should be rejected"
        );
        assert!(
            stderr.contains("cannot be used with"),
            "expected a clap conflict error for {conflicting:?}, got: {stderr}"
        );
    }
}
