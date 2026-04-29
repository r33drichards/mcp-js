//! End-to-end test for upstream MCP tool stubbing.
//!
//! Spins up two MCPJS instances over stdio:
//!
//!   * **upstream**: a plain MCPJS server (default tools).
//!   * **outer**: another MCPJS server configured to connect to `upstream`
//!     via `--mcp-server upstream=stdio:<server-bin>:--directory-path:...`.
//!
//! The outer server should advertise `mcp__upstream__run_js` (and the other
//! upstream tools) as stubs in `tools/list`. Calling one of those stubs
//! should return an instructional text result telling the caller to invoke
//! the tool from JavaScript via `run_js` + `mcp.callTool(...)`.

use serde_json::{json, Value};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::time::timeout;

mod common;

struct OuterServer {
    child: Child,
    stdin: tokio::process::ChildStdin,
    stdout: BufReader<tokio::process::ChildStdout>,
}

impl OuterServer {
    /// Start an MCPJS server on stdio whose `--mcp-server upstream=stdio:...`
    /// points at a second MCPJS subprocess.
    async fn start(outer_heap: &str, upstream_heap: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let server_bin = env!("CARGO_BIN_EXE_server");

        // The argument format is `name=stdio:command:arg1:arg2...`. We want
        // the upstream invocation to be `<server_bin> --directory-path <upstream_heap>`.
        let upstream_arg = format!(
            "upstream=stdio:{}:--directory-path:{}",
            server_bin, upstream_heap
        );

        let mut child = Command::new(server_bin)
            .args([
                "--directory-path",
                outer_heap,
                "--mcp-server",
                &upstream_arg,
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;

        let stdin = child.stdin.take().expect("stdin");
        let stdout = BufReader::new(child.stdout.take().expect("stdout"));

        // Give the outer server time to spawn the upstream + handshake.
        tokio::time::sleep(Duration::from_millis(1500)).await;

        Ok(Self { child, stdin, stdout })
    }

    async fn send(&mut self, msg: Value) -> Result<Value, Box<dyn std::error::Error>> {
        let s = format!("{}\n", serde_json::to_string(&msg)?);
        self.stdin.write_all(s.as_bytes()).await?;
        self.stdin.flush().await?;
        let mut line = String::new();
        timeout(Duration::from_secs(10), self.stdout.read_line(&mut line)).await??;
        Ok(serde_json::from_str(&line)?)
    }

    async fn send_notification(&mut self, msg: Value) -> Result<(), Box<dyn std::error::Error>> {
        let s = format!("{}\n", serde_json::to_string(&msg)?);
        self.stdin.write_all(s.as_bytes()).await?;
        self.stdin.flush().await?;
        Ok(())
    }

    async fn initialize(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let init = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "stub-e2e", "version": "1.0.0"}
            }
        });
        let _resp = self.send(init).await?;
        let initialized = json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        });
        self.send_notification(initialized).await?;
        Ok(())
    }

    async fn stop(mut self) {
        let _ = self.child.kill().await;
    }
}

fn tool_names(list_response: &Value) -> Vec<String> {
    list_response["result"]["tools"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|t| t.get("name").and_then(|n| n.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn outer_server_advertises_upstream_tools_as_stubs() -> Result<(), Box<dyn std::error::Error>> {
    let outer_heap = common::create_temp_heap_dir() + "-outer";
    let upstream_heap = common::create_temp_heap_dir() + "-upstream";
    std::fs::create_dir_all(&outer_heap).ok();
    std::fs::create_dir_all(&upstream_heap).ok();

    let mut server = OuterServer::start(&outer_heap, &upstream_heap).await?;
    server.initialize().await?;

    // Ask for the tool list.
    let list = server
        .send(json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }))
        .await?;

    let names = tool_names(&list);
    // Native tools still present.
    assert!(names.contains(&"run_js".to_string()), "native run_js missing: {:?}", names);
    // Upstream tools stubbed: at minimum, run_js from upstream.
    assert!(
        names.contains(&"mcp__upstream__run_js".to_string()),
        "expected mcp__upstream__run_js in tool list, got: {:?}",
        names,
    );
    // Several upstream tools should be stubbed (run_js, get_execution, list_executions, ...).
    let stub_count = names.iter().filter(|n| n.starts_with("mcp__upstream__")).count();
    assert!(stub_count >= 2, "expected multiple upstream stubs, got: {:?}", names);

    // Stub schemas should mirror the upstream tool's schema. For run_js
    // upstream tool, the stub should describe a `code` parameter.
    let stub = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t.get("name").and_then(|n| n.as_str()) == Some("mcp__upstream__run_js"))
        .expect("stub present");
    let schema = &stub["inputSchema"];
    assert!(
        schema["properties"]["code"].is_object(),
        "stub schema should have `code` property; got {}",
        serde_json::to_string_pretty(schema).unwrap_or_default(),
    );
    let desc = stub["description"].as_str().unwrap_or_default();
    assert!(desc.contains("run_js"), "description: {}", desc);
    assert!(desc.contains("mcp.callTool"), "description: {}", desc);

    server.stop().await;
    common::cleanup_heap_dir(&outer_heap);
    common::cleanup_heap_dir(&upstream_heap);
    Ok(())
}

#[tokio::test]
async fn calling_a_stub_returns_run_js_instructions() -> Result<(), Box<dyn std::error::Error>> {
    let outer_heap = common::create_temp_heap_dir() + "-outer2";
    let upstream_heap = common::create_temp_heap_dir() + "-upstream2";
    std::fs::create_dir_all(&outer_heap).ok();
    std::fs::create_dir_all(&upstream_heap).ok();

    let mut server = OuterServer::start(&outer_heap, &upstream_heap).await?;
    server.initialize().await?;

    let resp = server
        .send(json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "mcp__upstream__run_js",
                "arguments": {"code": "return 1 + 1;"}
            }
        }))
        .await?;

    // Expect a successful call_tool result whose first content block tells
    // the caller to invoke the tool from JS instead.
    assert_eq!(resp["result"]["isError"], json!(false));
    let text = resp["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(text.contains("mcp.callTool"), "stub call text: {}", text);
    assert!(text.contains("upstream"), "stub call text: {}", text);
    assert!(text.contains("run_js"), "stub call text: {}", text);
    assert!(text.contains("return 1 + 1"), "stub call text should echo args: {}", text);

    // Sanity: a non-stub native tool still dispatches normally.
    let resp = server
        .send(json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "list_executions",
                "arguments": {}
            }
        }))
        .await?;
    // Native tool responds with structured executions JSON, not the stub
    // instruction text.
    let text = resp["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(!text.contains("mcp.callTool"), "native list_executions should not return stub text: {}", text);

    server.stop().await;
    common::cleanup_heap_dir(&outer_heap);
    common::cleanup_heap_dir(&upstream_heap);
    Ok(())
}
