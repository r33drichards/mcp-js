//! End-to-end test for the Streamable HTTP *client* transport.
//!
//! Starts the `server` binary as a Streamable HTTP MCP server (`--http-port`,
//! served at `/mcp`) and attaches to it via `McpClientManager` configured with
//! the `Http` transport. This exercises the full client path: the initialize
//! handshake (which the server answers as `application/json` with an
//! `Mcp-Session-Id` header), `tools/list`, and `tools/call` (both answered as
//! `text/event-stream`).

mod common;

use std::time::Duration;

use server::engine::mcp_client::{McpClientManager, McpServerConfig, McpServerTransport};
use tokio::time::sleep;

const SERVER_NAME: &str = "upstream";

/// Find an available port by briefly binding to port 0.
fn find_available_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

/// Manages a Streamable HTTP MCP server child process for the test.
struct HttpServer {
    child: Option<tokio::process::Child>,
    mcp_url: String,
}

impl HttpServer {
    async fn start(heap_dir: &str) -> Result<Self, Box<dyn std::error::Error>> {
        use std::process::Stdio;
        use tokio::process::Command;

        let port = find_available_port();
        let child = Command::new(env!("CARGO_BIN_EXE_server"))
            .args(["--directory-path", heap_dir, "--http-port", &port.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;

        Ok(HttpServer {
            child: Some(child),
            mcp_url: format!("http://127.0.0.1:{}/mcp", port),
        })
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

/// Connect to the upstream over Streamable HTTP, retrying until the server is
/// ready (it may take a moment for the child process to bind the port).
async fn connect_with_retry(url: &str) -> Result<McpClientManager, String> {
    let mut last_err = String::from("no attempt made");
    for _ in 0..100 {
        let config = McpServerConfig {
            name: SERVER_NAME.to_string(),
            transport: McpServerTransport::Http {
                url: url.to_string(),
                headers: Default::default(),
            },
        };
        match McpClientManager::connect(vec![config]).await {
            Ok(manager) => return Ok(manager),
            Err(e) => {
                last_err = e;
                sleep(Duration::from_millis(100)).await;
            }
        }
    }
    Err(last_err)
}

#[tokio::test]
async fn streamable_http_client_handshake_list_and_call() {
    let heap_dir = common::create_temp_heap_dir();
    let mut server = HttpServer::start(&heap_dir)
        .await
        .expect("failed to start streamable http server");

    let manager = connect_with_retry(&server.mcp_url)
        .await
        .expect("failed to connect to streamable http server");

    // tools/list over the Streamable HTTP transport should surface run_js.
    let tools = manager
        .list_tools(Some(SERVER_NAME))
        .expect("list_tools failed");
    let tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    assert!(
        tool_names.contains(&"run_js"),
        "expected run_js in upstream tools, got: {:?}",
        tool_names
    );

    // tools/call: run_js schedules an async execution and returns its id.
    let mut args = serde_json::Map::new();
    args.insert("code".into(), serde_json::json!("console.log(40 + 2)"));
    let call = manager
        .call_tool(SERVER_NAME, "run_js", Some(args))
        .await
        .expect("call_tool(run_js) failed");
    let run_text = call["content"][0]["text"]
        .as_str()
        .expect("run_js response should have text content");
    let run_json: serde_json::Value =
        serde_json::from_str(run_text).expect("run_js response text should be JSON");
    let execution_id = run_json["execution_id"]
        .as_str()
        .expect("run_js response should include execution_id");
    assert!(
        !execution_id.starts_with("error:"),
        "run_js returned an error: {}",
        execution_id
    );

    // Poll get_execution until the run completes.
    let mut completed = false;
    for _ in 0..50 {
        let mut args = serde_json::Map::new();
        args.insert("execution_id".into(), serde_json::json!(execution_id));
        let status = manager
            .call_tool(SERVER_NAME, "get_execution", Some(args))
            .await
            .expect("call_tool(get_execution) failed");
        let status_text = status["content"][0]["text"]
            .as_str()
            .expect("get_execution response should have text content");
        let status_json: serde_json::Value =
            serde_json::from_str(status_text).expect("get_execution text should be JSON");
        if status_json["status"] == "completed" {
            completed = true;
            break;
        }
        sleep(Duration::from_millis(100)).await;
    }
    assert!(completed, "run_js execution did not complete in time");

    // The evaluated value is captured as console output, fetched separately.
    let mut out_args = serde_json::Map::new();
    out_args.insert("execution_id".into(), serde_json::json!(execution_id));
    let output = manager
        .call_tool(SERVER_NAME, "get_execution_output", Some(out_args))
        .await
        .expect("call_tool(get_execution_output) failed");
    let output_text = output["content"][0]["text"]
        .as_str()
        .expect("get_execution_output response should have text content");
    let output_json: serde_json::Value =
        serde_json::from_str(output_text).expect("get_execution_output text should be JSON");
    let console = output_json["data"].as_str().unwrap_or_default();
    assert!(
        console.contains("42"),
        "expected console output to contain 42, got: {:?}",
        output_json
    );

    server.stop().await;
    common::cleanup_heap_dir(&heap_dir);
}
