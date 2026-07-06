//! End-to-end regression test for per-session `/work` persistence keyed by the
//! MCP-standard transport session (`Mcp-Session-Id`) — no custom
//! `X-MCP-Session-Id` header (issues #199 / #200).
//!
//! A spec-compliant MCP client only knows the `Mcp-Session-Id` the server issues
//! at `initialize` and echoes on every subsequent request; it has no reason to
//! send the server-specific `X-MCP-Session-Id`. This test drives exactly that
//! client shape against a session-capable server (`--fs-store dir`, which
//! defaults to an allow-all `/work` policy) and asserts that a file written to
//! `/work` in one `run_js` call is readable in the next call of the same
//! session — with only the standard session header in play.

use reqwest::Client;
use serde_json::{json, Value};
use std::process::Stdio;
use tempfile::TempDir;
use tokio::process::Command;
use tokio::time::{sleep, Duration};

// ── Server harness ─────────────────────────────────────────────────────────

struct HttpServer {
    child: Option<tokio::process::Child>,
    base_url: String,
    // Kept alive for the lifetime of the server; dropped (removed) on teardown.
    _db_dir: TempDir,
}

impl HttpServer {
    async fn start() -> Result<Self, Box<dyn std::error::Error>> {
        let port = find_available_port();
        let db_dir = tempfile::tempdir()?;
        // Session-capable mode: fs persistence on (`--fs-store dir`). With no
        // `--policies-json`, the fs surface defaults to an allow-all `/work`
        // policy, so this is a persistence test, not a policy test. The session
        // db (and, by default, the fs blob store under it) live in a temp dir so
        // parallel tests don't collide on the shared default path.
        let child = Command::new(env!("CARGO_BIN_EXE_server"))
            .args([
                "--http-port",
                &port.to_string(),
                "--bind-host",
                "127.0.0.1",
                "--fs-store",
                "dir",
                "--session-db-path",
                db_dir.path().to_str().unwrap(),
            ])
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
                return Ok(Self {
                    child: Some(child),
                    base_url,
                    _db_dir: db_dir,
                });
            }
            sleep(Duration::from_millis(100)).await;
        }
        Err("server did not become ready within 15s".into())
    }

    fn mcp_url(&self) -> String {
        format!("{}/mcp", self.base_url)
    }

    async fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill().await;
            let _ = child.wait().await;
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

// ── MCP client helpers ─────────────────────────────────────────────────────

const ACCEPT: &str = "application/json, text/event-stream";

fn client() -> Client {
    Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("client")
}

/// Extract the JSON-RPC object from a Streamable HTTP POST response body, which
/// may be SSE-framed (`data:` lines) or a single JSON object.
fn parse_rpc(body: &str) -> Value {
    if body.contains("data:") {
        let mut data = String::new();
        for line in body.lines() {
            if let Some(rest) = line.strip_prefix("data:") {
                data.push_str(rest.strip_prefix(' ').unwrap_or(rest));
            }
        }
        serde_json::from_str(&data).unwrap_or(Value::Null)
    } else {
        serde_json::from_str(body).unwrap_or(Value::Null)
    }
}

/// Initialize an MCP session as a spec-compliant client would: NO custom
/// `X-MCP-Session-Id` header. Returns the server-issued `Mcp-Session-Id`.
async fn initialize(client: &Client, url: &str) -> String {
    let resp = client
        .post(url)
        .header("Accept", ACCEPT)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "persistence-e2e", "version": "1.0.0" }
            }
        }))
        .send()
        .await
        .expect("initialize request");
    assert!(resp.status().is_success(), "initialize status: {}", resp.status());
    let session_id = resp
        .headers()
        .get("mcp-session-id")
        .expect("mcp-session-id header on initialize")
        .to_str()
        .unwrap()
        .to_string();

    client
        .post(url)
        .header("Accept", ACCEPT)
        .header("mcp-session-id", &session_id)
        .json(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
        .send()
        .await
        .expect("initialized notification");

    session_id
}

/// Call a tool over Streamable HTTP and return the JSON object carried in the
/// tool result's text content (the server wraps each tool's JSON body as a
/// single text content part).
async fn call_tool(
    client: &Client,
    url: &str,
    session: &str,
    id: u64,
    name: &str,
    arguments: Value,
) -> Value {
    let resp = client
        .post(url)
        .header("Accept", ACCEPT)
        .header("mcp-session-id", session)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": { "name": name, "arguments": arguments }
        }))
        .send()
        .await
        .expect("tools/call request");
    assert!(resp.status().is_success(), "tools/call status: {}", resp.status());
    let body = resp.text().await.expect("tools/call body");
    let rpc = parse_rpc(&body);
    let text: String = rpc["result"]["content"]
        .as_array()
        .map(|parts| {
            parts
                .iter()
                .filter(|p| p["type"] == "text")
                .filter_map(|p| p["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    serde_json::from_str(&text).unwrap_or(Value::Null)
}

/// Submit `code` via async `run_js`, poll `get_execution` to a terminal state,
/// and return `(status, output)` where `output` is the console output on success
/// or the error message on failure.
async fn run_js(client: &Client, url: &str, session: &str, code: &str) -> (String, String) {
    let submit = call_tool(client, url, session, 2, "run_js", json!({ "code": code })).await;
    let exec_id = submit["execution_id"]
        .as_str()
        .unwrap_or_else(|| panic!("execution_id missing from run_js result: {submit}"))
        .to_string();

    for _ in 0..300 {
        let meta = call_tool(
            client,
            url,
            session,
            3,
            "get_execution",
            json!({ "execution_id": exec_id }),
        )
        .await;
        let status = meta["status"].as_str().unwrap_or("").to_string();
        if matches!(status.as_str(), "completed" | "failed" | "cancelled" | "timed_out") {
            let out = call_tool(
                client,
                url,
                session,
                4,
                "get_execution_output",
                json!({ "execution_id": exec_id, "byte_limit": 4000 }),
            )
            .await;
            let data = out["data"].as_str().unwrap_or("").to_string();
            let error = meta["error"].as_str().unwrap_or("").to_string();
            let output = if data.is_empty() { error } else { data };
            return (status, output);
        }
        sleep(Duration::from_millis(100)).await;
    }
    panic!("run_js did not reach a terminal state in time");
}

// ── Test ────────────────────────────────────────────────────────────────────

/// A file written to `/work` (with `fs` omitted) in one `run_js` call is
/// readable in the next call of the SAME session — using only the standard
/// `Mcp-Session-Id`, no custom `X-MCP-Session-Id`. Regression for #199 / #200.
#[tokio::test]
async fn work_persists_across_calls_with_standard_mcp_session() {
    let mut server = HttpServer::start().await.expect("server start");
    let c = client();
    let url = server.mcp_url();

    let session = initialize(&c, &url).await;

    // Call 1: write to /work. `fs` omitted → the session's fs is mounted
    // automatically. Before the fix this mounted nothing and failed with ENOENT.
    let (write_status, write_out) = run_js(
        &c,
        &url,
        &session,
        "await fs.writeFile('/work/a.txt', 'persisted!'); console.log('wrote')",
    )
    .await;
    assert_eq!(
        write_status, "completed",
        "write to /work should succeed, got output: {write_out}"
    );

    // Call 2: read it back in the same session, `fs` omitted again.
    let (read_status, read_out) = run_js(
        &c,
        &url,
        &session,
        "console.log(await fs.readFile('/work/a.txt', 'utf8'))",
    )
    .await;
    assert_eq!(
        read_status, "completed",
        "read from /work should succeed, got output: {read_out}"
    );
    assert!(
        read_out.contains("persisted!"),
        "/work file written in call 1 should persist into call 2, got: {read_out}"
    );

    server.stop().await;
}
