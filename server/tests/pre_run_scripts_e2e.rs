/// End-to-end tests for --init-script / --pre-run-script on the spawned
/// server binary:
///
///   - inline and @file values reach executions via the REST sidecar
///   - TypeScript in a script file is stripped at startup
///   - an invalid (unparsable) script fails startup with a flag-prefixed error
use reqwest::Client;
use serde_json::{Value, json};
use std::process::Stdio;
use tokio::process::Command;
use tokio::time::{Duration, sleep};

// ── Server helper ────────────────────────────────────────────────────────

struct HttpServer {
    child: Option<tokio::process::Child>,
    pub base_url: String,
}

impl HttpServer {
    async fn start(extra_args: &[&str]) -> Result<Self, Box<dyn std::error::Error>> {
        let port = find_available_port();

        let child = Command::new(env!("CARGO_BIN_EXE_server"))
            .args(["--http-port", &port.to_string()])
            .args(extra_args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;

        let base_url = format!("http://127.0.0.1:{}", port);
        let client = Client::new();
        let health = format!("{}/api/executions", base_url);

        for _ in 0..150 {
            if client
                .get(&health)
                .timeout(Duration::from_millis(100))
                .send()
                .await
                .is_ok()
            {
                return Ok(Self { child: Some(child), base_url });
            }
            sleep(Duration::from_millis(100)).await;
        }
        Err("Server did not become ready within 15s".into())
    }

    async fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill().await;
            let _ = child.wait().await;
            sleep(Duration::from_millis(500)).await;
        }
    }
}

impl Drop for HttpServer {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            let _ = child.start_kill();
        }
    }
}

fn find_available_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Submit code for execution and return the execution_id.
async fn submit_code(client: &Client, base_url: &str, code: &str) -> String {
    let resp = client
        .post(format!("{}/api/exec", base_url))
        .json(&json!({ "code": code }))
        .send()
        .await
        .expect("POST /api/exec failed");
    assert_eq!(resp.status(), 202, "Expected 202 Accepted");
    let body: Value = resp.json().await.expect("Invalid JSON from /api/exec");
    body["execution_id"]
        .as_str()
        .expect("Missing execution_id")
        .to_string()
}

/// Poll until an execution reaches a terminal state.
async fn poll_until_done(client: &Client, base_url: &str, id: &str) -> Value {
    let url = format!("{}/api/executions/{}", base_url, id);
    for _ in 0..100 {
        let resp = client.get(&url).send().await.expect("GET execution failed");
        let body: Value = resp.json().await.expect("Invalid JSON");
        let status = body["status"].as_str().unwrap_or("");
        if status != "running" {
            return body;
        }
        sleep(Duration::from_millis(50)).await;
    }
    panic!("Execution {} did not finish within 5s", id);
}

// ── Tests ────────────────────────────────────────────────────────────────

/// A TypeScript init script from a file and an inline pre-run script both
/// reach executions; the TS types are stripped at startup.
#[tokio::test]
async fn scripts_reach_executions_via_flags() -> Result<(), Box<dyn std::error::Error>> {
    let dir = std::env::temp_dir().join(format!("mcp-pre-run-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let script_path = dir.join("init.ts");
    std::fs::write(
        &script_path,
        "const base: number = 42;\nglobalThis.base = base;\n",
    )?;

    let init_arg = format!("@{}", script_path.display());
    let mut server = HttpServer::start(&[
        "--init-script",
        &init_arg,
        "--pre-run-script",
        "globalThis.pre = true",
    ])
    .await?;
    let client = Client::new();

    let id = submit_code(
        &client,
        &server.base_url,
        "if (globalThis.base !== 42) throw new Error('no init'); \
         if (globalThis.pre !== true) throw new Error('no pre-run');",
    )
    .await;
    let body = poll_until_done(&client, &server.base_url, &id).await;
    assert_eq!(body["status"], "completed", "got: {}", body);

    server.stop().await;
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

/// An unparsable script fails startup with an error naming the flag, instead
/// of failing every execution later.
#[tokio::test]
async fn invalid_script_fails_startup() -> Result<(), Box<dyn std::error::Error>> {
    let port = find_available_port();
    let output = Command::new(env!("CARGO_BIN_EXE_server"))
        .args([
            "--http-port",
            &port.to_string(),
            "--init-script",
            "const x: = broken(",
        ])
        .stdin(Stdio::null())
        .output()
        .await?;

    assert!(!output.status.success(), "server should refuse to start");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("--init-script"), "stderr should name the flag: {}", stderr);
    Ok(())
}
