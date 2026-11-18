//! Real server startup, HTTP/MCP operations and process restart with SQLite.
#[path = "support/s3.rs"]
mod s3;
use mcp_metadata_store::{MetadataStore, SqliteStore};
use reqwest::{Client, StatusCode};
use serde_json::{Value, json};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{
    process::{Child, Command},
    time::{sleep, timeout},
};

fn command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_server"));
    // Keep developer/CI configuration and credentials out of the child.
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("MCP_V8_") {
            command.env_remove(key);
        }
    }
    command
        .env_remove("MCP_SCHEDULED_WORKER_CONFIG")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .kill_on_drop(true);
    command
}

struct Server {
    child: Child,
    client: Client,
    url: String,
    s3: Option<s3::S3>,
}

impl Server {
    async fn start(dir: &Path, selection: &str) -> Self {
        let s3 = s3::S3::start().await;
        let mut server = Self::start_with_s3(dir, selection, Some(&s3)).await;
        server.s3 = Some(s3);
        server
    }

    async fn start_with_s3(dir: &Path, selection: &str, s3: Option<&s3::S3>) -> Self {
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let port = if selection == "sled" {
            let saved = dir.join("server-port");
            match std::fs::read_to_string(&saved) {
                Ok(value) => value.parse::<u16>().unwrap(),
                Err(_) => {
                    std::fs::write(saved, port.to_string()).unwrap();
                    port
                }
            }
        } else {
            port
        };
        let database = dir.join("metadata.sqlite");
        let mut command = command();
        command
            .args([
                "--http-port",
                &port.to_string(),
                "--bind-host",
                "127.0.0.1",
                "--heap-store",
                "dir",
                "--fs-store",
                "dir",
                "--session-db-path",
            ])
            .arg(dir.join("local"))
            .arg("--heap-dir")
            .arg(dir.join("heaps"))
            .arg("--fs-dir")
            .arg(dir.join("blobs"));
        match selection {
            "sled" => {}
            "flag" => {
                command.arg("--sqlite-path").arg(&database);
            }
            "env" => {
                command.env("MCP_V8_SQLITE_PATH", &database);
            }
            "config" => {
                let config = dir.join("server.json");
                std::fs::write(&config, json!({"sqlite_path":database}).to_string()).unwrap();
                command.arg("--config").arg(config);
            }
            _ => panic!("unknown backend selection"),
        }
        if let Some(s3) = s3 {
            s3.configure(&mut command);
        }
        let mut server = Self {
            child: command.spawn().unwrap(),
            client: Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .unwrap(),
            url: format!("http://127.0.0.1:{port}"),
            s3: None,
        };
        timeout(Duration::from_secs(15), async {
            loop {
                assert!(
                    server.child.try_wait().unwrap().is_none(),
                    "server exited before readiness"
                );
                if let Ok(response) = server
                    .client
                    .get(format!("{}/api/capabilities", server.url))
                    .send()
                    .await
                {
                    if response.status().is_success() {
                        break;
                    }
                }
                sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("server startup deadline");
        if selection != "sled" {
            assert!(
                database.is_file(),
                "bootstrap must create the selected database"
            );
            assert!(
                !dir.join("local/heap-tags").exists(),
                "tags must not fall back to sled"
            );
            assert!(
                !dir.join("local/fs-labels").exists(),
                "labels must not fall back to sled"
            );
        }
        server
    }

    async fn get(&self, path: &str) -> Value {
        self.client
            .get(format!("{}{path}", self.url))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap()
    }

    async fn post(&self, path: &str, body: Value, status: StatusCode) -> Value {
        let response = self
            .client
            .post(format!("{}{path}", self.url))
            .json(&body)
            .send()
            .await
            .unwrap();
        let actual = response.status();
        let body: Value = response.json().await.unwrap();
        assert_eq!(actual, status, "{path}: {body}");
        body
    }

    async fn execute(&self, body: Value) -> Value {
        let accepted = self.post("/api/exec", body, StatusCode::ACCEPTED).await;
        timeout(Duration::from_secs(20), async {
            loop {
                let info = self
                    .get(&format!(
                        "/api/executions/{}",
                        accepted["execution_id"].as_str().unwrap()
                    ))
                    .await;
                match info["status"].as_str().unwrap() {
                    "completed" => return info,
                    "running" => sleep(Duration::from_millis(20)).await,
                    _ => panic!("execution failed: {info}"),
                }
            }
        })
        .await
        .expect("execution deadline")
    }

    async fn tags(&self, heap: &Value) -> Value {
        self.tool("get_heap_tags", json!({"heap":heap})).await
    }

    async fn tool(&self, name: &str, arguments: Value) -> Value {
        let response = self.client.post(format!("{}/mcp", self.url))
            .header("Accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", "2026-07-28")
            .header("Mcp-Method", "tools/call").header("Mcp-Name", name)
            .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{
                "name":name,"arguments":arguments,"_meta":{
                    "io.modelcontextprotocol/protocolVersion":"2026-07-28",
                    "io.modelcontextprotocol/clientInfo":{"name":"sqlite-integration","version":"1"},
                    "io.modelcontextprotocol/clientCapabilities":{}
                }
            }})).send().await.unwrap().error_for_status().unwrap();
        let text = response.text().await.unwrap();
        let raw = if text.contains("data:") {
            text.lines()
                .filter_map(|line| line.strip_prefix("data:"))
                .collect::<String>()
        } else {
            text
        };
        let reply: Value = serde_json::from_str(&raw).unwrap();
        assert!(reply.get("error").is_none(), "{reply}");
        assert!(
            !reply["result"]["isError"].as_bool().unwrap_or(false),
            "{reply}"
        );
        serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
    }

    #[cfg(unix)]
    async fn stop_for_restart(mut self) {
        // Let sled close and flush its buffered metadata. This regression is
        // about module identities across restart, not unflushed crash recovery.
        let pid = self.child.id().expect("live child") as libc::pid_t;
        assert_eq!(unsafe { libc::kill(pid, libc::SIGINT) }, 0);
        let status = timeout(Duration::from_secs(10), self.child.wait())
            .await
            .expect("graceful shutdown deadline")
            .unwrap();
        assert!(status.success(), "graceful server shutdown: {status}");
    }

    async fn stop(mut self) {
        // Kill/reap rather than sharing an Engine/store handle across "restart".
        self.child.kill().await.unwrap();
        self.child.wait().await.unwrap();
    }
}

#[tokio::test]
async fn sqlite_runtime_records_console_artifacts_and_upload_grants_survive_restart() {
    let s3 = s3::S3::start().await;
    for selection in ["flag", "env", "config"] {
        let dir = tempfile::tempdir().unwrap();
        let server = Server::start_with_s3(dir.path(), selection, Some(&s3)).await;
        let settled = server.tool("run_js", json!({"wait":true,"code":
            "console.log('C'.repeat(150000)); console.log('tail'); \
             artifact('saved.bin', 'application/octet-stream', new Uint8Array(500000).fill(7)); \
             export const __mcp_http_result = 'R'.repeat(150000);"
        })).await;
        assert_eq!(settled["status"], "completed", "{settled}");
        let id = settled["execution_id"].as_str().unwrap();
        let execution = server.get(&format!("/api/executions/{id}")).await;
        assert_eq!(execution["result"].as_str().unwrap().len(), 150_000);
        assert_eq!(execution["artifacts"][0]["key"], "saved.bin");
        let output_path = format!("/api/executions/{id}/output?byte_offset=10&byte_limit=32");
        let output = server.get(&output_path).await;
        assert_eq!(output["data"], "C".repeat(32));
        assert_eq!(output["has_more"], true);
        let grant = server.tool("get_artifact_upload_url", json!({
            "key":"uploaded.bin", "mime_type":"application/octet-stream", "expires_in_secs":300
        })).await;
        let grant_path = grant["path"].as_str().unwrap();
        server.stop().await;

        if selection != "sled" {
            let store = SqliteStore::open(dir.path().join("metadata.sqlite"), "runtime").unwrap();
            let row = store.get("execution_records", id.as_bytes()).await.unwrap();
            assert!(
                row.is_some(),
                "{selection}: execution record missing from SQLite"
            );
            let row = row.unwrap();
            assert_eq!(row[0], 1, "execution result must be inline, without S3");
            assert!(row.len() > 128 * 1024);
            let artifact = store.get("artifacts", b"saved.bin").await.unwrap().unwrap();
            assert!(artifact.len() < 128, "artifact bytes must be in S3");
            assert!(
                !store
                    .scan(&format!("ex:{id}"), b"", false, None)
                    .await
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(
                store
                    .scan("artifact_uploads", b"", false, None)
                    .await
                    .unwrap()
                    .len(),
                1
            );
            for (_, reference) in store
                .scan(&format!("ex:{id}"), b"", false, None)
                .await
                .unwrap()
            {
                assert!(
                    reference.len() < 128,
                    "even small console pages must be in S3"
                );
            }
            drop(store);
        }
        if selection != "sled" {
            if let Ok(entries) = std::fs::read_dir(dir.path().join("local")) {
                for entry in entries {
                    assert!(
                        !entry
                            .unwrap()
                            .file_name()
                            .to_string_lossy()
                            .starts_with("executions"),
                        "{selection}: SQLite mode must not open a sled execution database"
                    );
                }
            }
        }
        let server = Server::start_with_s3(dir.path(), selection, Some(&s3)).await;
        assert_eq!(
            server.get(&format!("/api/executions/{id}")).await,
            execution
        );
        assert_eq!(server.get(&output_path).await, output);
        let listed = server.get("/api/executions").await;
        assert!(
            listed["executions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["execution_id"] == id)
        );
        let saved = server
            .client
            .get(format!("{}/api/artifacts/saved.bin", server.url))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .bytes()
            .await
            .unwrap();
        assert_eq!(saved.as_ref(), vec![7; 500_000]);

        // Independent server processes/SQLite connections must share one winner.
        let peer = Server::start_with_s3(dir.path(), selection, Some(&s3)).await;
        let upload_url = format!("{}{grant_path}", server.url);
        let peer_url = format!("{}{grant_path}", peer.url);
        let upload = vec![8; 500_000];
        let (first, second) = tokio::join!(
            server.client.put(&upload_url).body(upload.clone()).send(),
            peer.client.put(&peer_url).body(upload.clone()).send()
        );
        let mut statuses = [
            first.unwrap().status().as_u16(),
            second.unwrap().status().as_u16(),
        ];
        statuses.sort_unstable();
        assert_eq!(statuses, [200, 404]);
        peer.stop().await;
        server.stop().await;

        let server = Server::start_with_s3(dir.path(), selection, Some(&s3)).await;
        assert_eq!(
            server
                .client
                .put(format!("{}{grant_path}", server.url))
                .body("must not overwrite")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
        let uploaded = server
            .client
            .get(format!("{}/api/artifacts/uploaded.bin", server.url))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .bytes()
            .await
            .unwrap();
        assert_eq!(uploaded.as_ref(), upload);
        assert_eq!(
            server.get(&format!("/api/executions/{id}")).await,
            execution
        );
        server.stop().await;
    }
}

#[tokio::test]
async fn sqlite_preserves_sessions_tags_and_label_reflogs_across_server_restart() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::start(dir.path(), "flag").await;
    let first = server.execute(json!({"session":"sqlite-session","fs":"main",
        "tags":{"backend":"sqlite"},"code":"globalThis.counter = 41; await fs.writeFile('/note.txt', 'persisted');"})).await;
    assert!(
        first["heap"].is_string() && first["fs"].is_string(),
        "{first}"
    );
    server
        .post(
            "/api/fs/push",
            json!({"label":"main","ca_id":first["fs"],"message":"initial"}),
            StatusCode::OK,
        )
        .await;
    let history = server.get("/api/sessions/sqlite-session/snapshots").await;
    assert_eq!(history.as_array().unwrap().len(), 1);
    assert_eq!(history[0]["output_heap"], first["heap"]);
    assert_eq!(history[0]["output_fs"], first["fs"]);
    let tags = server.tags(&first["heap"]).await;
    assert_eq!(tags["tags"]["backend"], "sqlite", "{tags}");
    let log = server.get("/api/fs/labels/main/log").await;
    assert_eq!(log["log"].as_array().unwrap().len(), 1);
    server.stop().await;

    // Positive evidence for all three namespaces rules out accidental sled use.
    for namespace in ["sessions", "tags", "labels"] {
        let store = SqliteStore::open(dir.path().join("metadata.sqlite"), namespace).unwrap();
        assert!(
            !store.tree_names().await.unwrap().is_empty(),
            "{namespace} not in SQLite"
        );
    }
    let server = Server::start(dir.path(), "flag").await;
    assert_eq!(
        server.get("/api/sessions/sqlite-session/snapshots").await,
        history
    );
    assert_eq!(server.tags(&first["heap"]).await, tags);
    assert_eq!(server.get("/api/fs/labels/main/log").await, log);
    assert_eq!(
        server.get("/api/fs/labels/main").await["ca_id"],
        first["fs"]
    );
    // Native session reads/writes resolve the persisted session head through
    // bootstrap's SQLite log, then load blobs from the configured filesystem.
    let content = server
        .client
        .get(format!(
            "{}/api/sessions/sqlite-session/files/note.txt",
            server.url
        ))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(content, "persisted");
    let response = server
        .client
        .put(format!(
            "{}/api/sessions/sqlite-session/files/note.txt",
            server.url
        ))
        .body("updated")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let snapshots = server.get("/api/sessions/sqlite-session/snapshots").await;
    assert_eq!(snapshots.as_array().unwrap().len(), 2);
    assert_eq!(
        snapshots[1]["output_heap"], first["heap"],
        "native writes preserve the heap reference"
    );
    let second = json!({"fs":snapshots[1]["output_fs"]});
    assert_ne!(second["fs"], first["fs"]);
    // A stale CAS must append no reflog record and leave the head unchanged.
    server
        .post(
            "/api/fs/push",
            json!({"label":"main","ca_id":second["fs"],"expected":"00".repeat(32)}),
            StatusCode::CONFLICT,
        )
        .await;
    assert_eq!(server.get("/api/fs/labels/main/log").await, log);
    assert_eq!(
        server.get("/api/fs/labels/main").await["ca_id"],
        first["fs"]
    );
    server.post("/api/fs/push", json!({"label":"main","ca_id":second["fs"],"expected":first["fs"],"message":"after restart"}), StatusCode::OK).await;
    assert_eq!(
        server.get("/api/fs/labels/main").await["ca_id"],
        second["fs"]
    );
    assert_eq!(
        server.get("/api/fs/labels/main/log").await["log"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        server
            .get("/api/sessions/sqlite-session/snapshots")
            .await
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let updated_history = server.get("/api/sessions/sqlite-session/snapshots").await;
    let updated_log = server.get("/api/fs/labels/main/log").await;
    server.stop().await;
    let server = Server::start(dir.path(), "flag").await;
    assert_eq!(
        server.get("/api/sessions/sqlite-session/snapshots").await,
        updated_history
    );
    assert_eq!(server.get("/api/fs/labels/main/log").await, updated_log);
    assert_eq!(
        server.get("/api/fs/labels/main").await["ca_id"],
        second["fs"]
    );
    let content = server
        .client
        .get(format!(
            "{}/api/sessions/sqlite-session/files/note.txt",
            server.url
        ))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(content, "updated");
    server.stop().await;
}

#[tokio::test]
async fn sqlite_selection_via_flag_environment_and_config_writes_the_selected_database() {
    for selection in ["flag", "env", "config"] {
        let dir = tempfile::tempdir().unwrap();
        let server = Server::start(dir.path(), selection).await;
        let ca = "ab".repeat(32);
        server
            .post(
                "/api/fs/labels",
                json!({"name":"selected","ca_id":ca}),
                StatusCode::OK,
            )
            .await;
        server.stop().await;
        let store = SqliteStore::open(dir.path().join("metadata.sqlite"), "labels").unwrap();
        assert_eq!(
            store.get("fs_label_heads", b"selected").await.unwrap(),
            Some(vec![0xab; 32]),
            "{selection}"
        );
    }
}

#[tokio::test]
async fn sqlite_open_failure_aborts_server_without_sled_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let mut command = command();
    command
        .args(["--fs-store", "dir", "--sqlite-path"])
        .arg(dir.path().join("missing-parent/metadata.sqlite"))
        .arg("--session-db-path")
        .arg(dir.path().join("local"))
        .stderr(Stdio::piped());
    let output = timeout(Duration::from_secs(10), command.output())
        .await
        .expect("startup must fail promptly")
        .unwrap();
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("unable to open database"), "{error}");
    assert!(
        !dir.path().join("local").exists(),
        "failed explicit selection must not open sled"
    );
}

// The process-local entry-module counter used to reset to main_0.js on each
// restart, colliding with an entry already present in the saved module map.
#[cfg(unix)]
async fn heap_restore_roundtrip(selection: &str) {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::start(dir.path(), selection).await;
    let first = server
        .execute(json!({"session":"roundtrip", "fs":"main",
        "code":"globalThis.counter = 41; await fs.writeFile('/note.txt', 'heap-roundtrip');"}))
        .await;
    assert!(first["heap"].is_string(), "{first}");
    server.stop_for_restart().await;

    let server = Server::start(dir.path(), selection).await;
    // Explicit handle after process restart, then implicit session restore in
    // the same process. Every call takes another snapshot of the restored heap.
    for (expected, explicit) in [(41, true), (42, false)] {
        let mut body = json!({"session":"roundtrip", "code": format!(
            "if (counter !== {expected}) throw new Error('counter lost: ' + counter); \
             if (await fs.readFile('/note.txt', 'utf8') !== 'heap-roundtrip') throw new Error('fs lost'); \
             counter++; export const __mcp_http_result = JSON.stringify({{ counter }});"
        )});
        if explicit {
            body["heap"] = first["heap"].clone();
        }
        let result = server.execute(body).await;
        let value: Value = serde_json::from_str(result["result"].as_str().unwrap()).unwrap();
        assert_eq!(value["counter"], expected + 1);
        assert!(result["heap"].is_string());
    }
    server.stop_for_restart().await;

    let server = Server::start(dir.path(), selection).await;
    let last = server.execute(json!({"session":"roundtrip", "code":
        "if (counter !== 43) throw new Error('second restart lost counter'); counter++; export const __mcp_http_result = JSON.stringify({ counter });"})).await;
    let value: Value = serde_json::from_str(last["result"].as_str().unwrap()).unwrap();
    assert_eq!(value["counter"], 44);
    let history = server.get("/api/sessions/roundtrip/snapshots").await;
    assert_eq!(history.as_array().unwrap().len(), 4);
    for pair in history.as_array().unwrap().windows(2) {
        assert_eq!(pair[1]["input_heap"], pair[0]["output_heap"]);
    }
    server.stop_for_restart().await;
}

#[cfg(unix)]
#[tokio::test]
async fn sqlite_heap_restore_survives_repeated_server_restarts() {
    heap_restore_roundtrip("flag").await;
}

#[cfg(unix)]
#[tokio::test]
async fn sled_heap_restore_survives_repeated_server_restarts() {
    heap_restore_roundtrip("sled").await;
}

#[tokio::test]
async fn runtime_s3_required_and_failed_upload_never_replaces_database_reference() {
    use std::sync::atomic::Ordering;
    for selection in ["flag", "sled"] {
        let dir = tempfile::tempdir().unwrap();
        let server = Server::start_with_s3(dir.path(), selection, None).await;
        let response = server
            .client
            .put(format!("{}/api/artifacts/tiny.txt", server.url))
            .header("content-type", "text/plain")
            .body("tiny")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert!(response.text().await.unwrap().contains("S3"));
        server.stop().await;

        let s3 = s3::S3::start().await;
        let server = Server::start_with_s3(dir.path(), selection, Some(&s3)).await;
        let result = server.tool("run_js", json!({"wait":true, "code":
            "console.log('tiny'); artifact('tiny.txt', 'text/plain', new TextEncoder().encode('tiny'));"
        })).await;
        assert_eq!(result["status"], "completed", "{result}");
        let id = result["execution_id"].as_str().unwrap();
        assert!(
            server.get(&format!("/api/executions/{id}/output")).await["data"]
                .as_str()
                .unwrap()
                .contains("tiny")
        );
        assert!(
            s3.objects
                .values
                .lock()
                .unwrap()
                .values()
                .any(|value| value.ends_with(b"tiny"))
        );
        s3.objects.fail_put.store(true, Ordering::SeqCst);
        let response = server
            .client
            .put(format!("{}/api/artifacts/tiny.txt", server.url))
            .header("content-type", "text/plain")
            .body("replacement")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let response = server
            .client
            .get(format!("{}/api/artifacts/tiny.txt", server.url))
            .send()
            .await
            .unwrap();
        assert_eq!(response.text().await.unwrap(), "tiny");
        s3.objects.fail_put.store(false, Ordering::SeqCst);
        server.stop().await;
        // Same S3 objects, independent server and local database handles.
        let server = Server::start_with_s3(dir.path(), selection, Some(&s3)).await;
        let response = server
            .client
            .get(format!("{}/api/artifacts/tiny.txt", server.url))
            .send()
            .await
            .unwrap();
        assert_eq!(response.text().await.unwrap(), "tiny");
        s3.objects.values.lock().unwrap().clear();
        let response = server
            .client
            .get(format!("{}/api/artifacts/tiny.txt", server.url))
            .send()
            .await
            .unwrap();
        assert!(
            !response.status().is_success(),
            "missing S3 objects must not fall back"
        );
        server.stop().await;
    }
}
