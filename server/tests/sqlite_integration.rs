//! Real server startup, HTTP/MCP operations and process restart with SQLite.
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
}

impl Server {
    async fn start(dir: &Path, selection: &str) -> Self {
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
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
        let mut server = Self {
            child: command.spawn().unwrap(),
            client: Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .unwrap(),
            url: format!("http://127.0.0.1:{port}"),
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
        let response = self.client.post(format!("{}/mcp", self.url))
            .header("Accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", "2026-07-28")
            .header("Mcp-Method", "tools/call").header("Mcp-Name", "get_heap_tags")
            .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{
                "name":"get_heap_tags","arguments":{"heap":heap},"_meta":{
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

    async fn stop(mut self) {
        // Kill/reap rather than sharing an Engine/store handle across "restart".
        self.child.kill().await.unwrap();
        self.child.wait().await.unwrap();
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
