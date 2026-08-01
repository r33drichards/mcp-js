//! End-to-end test for upstream MCP tool stubbing.
//!
//! Spins up two MCPJS instances over stdio:
//!
//!   * **upstream**: a plain MCPJS server (default tools).
//!   * **outer**: another MCPJS server configured to connect to `upstream`
//!     via `--mcp-server upstream=stdio:<server-bin>:--directory-path:...`.
//!
//! The outer server should advertise `runjs__upstream__run_js` (and the other
//! upstream tools) as stubs in `tools/list`. Calling one of those stubs
//! should return an instructional text result telling the caller to invoke
//! the tool from JavaScript via `run_js` + `await mcp.upstream.<tool>(args)`.
//! Also exercises the proxy namespaces end-to-end from inside the sandbox.

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
    /// points at a second MCPJS subprocess. `extra_args` lets a test pass
    /// additional flags (e.g. `--mcp-stubs false` or
    /// `--mcp-stub-prefix rj_`).
    async fn start_with_args(
        outer_heap: &str,
        upstream_heap: &str,
        extra_args: &[&str],
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let server_bin = env!("CARGO_BIN_EXE_server");

        // The argument format is `name=stdio:command:arg1:arg2...`. We want the
        // upstream invocation to be `<server_bin> --heap-store dir --heap-dir <upstream_heap>`.
        let upstream_arg = format!(
            "upstream=stdio:{}:--heap-store:dir:--heap-dir:{}",
            server_bin, upstream_heap
        );

        let mut args: Vec<String> = vec![
            "--heap-store".to_string(),
            "dir".to_string(),
            "--heap-dir".to_string(),
            outer_heap.to_string(),
            "--mcp-server".to_string(),
            upstream_arg,
        ];
        args.extend(extra_args.iter().map(|s| s.to_string()));

        let mut child = Command::new(server_bin)
            .args(&args)
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

    async fn start(outer_heap: &str, upstream_heap: &str) -> Result<Self, Box<dyn std::error::Error>> {
        Self::start_with_args(outer_heap, upstream_heap, &[]).await
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

    /// Send a run_js tool call on the OUTER server and poll get_execution
    /// until it finishes. Returns the completed execution info.
    async fn run_js_and_wait(
        &mut self,
        id: u64,
        code: &str,
    ) -> Result<Value, Box<dyn std::error::Error>> {
        let msg = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": {
                "name": "run_js",
                "arguments": { "code": code }
            }
        });
        let response = self.send(msg).await?;
        let exec_id = common::extract_execution_id(&response)
            .ok_or("run_js response should contain execution_id")?;

        for i in 0..200 {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let poll = json!({
                "jsonrpc": "2.0",
                "id": 10000 + id * 1000 + i,
                "method": "tools/call",
                "params": {
                    "name": "get_execution",
                    "arguments": { "execution_id": exec_id }
                }
            });
            let poll_resp = self.send(poll).await?;
            if let Some(mut info) = common::extract_execution_info(&poll_resp) {
                match info["status"].as_str() {
                    Some("completed") | Some("failed") | Some("timed_out") | Some("cancelled") => {
                        info["execution_id"] = json!(exec_id);
                        return Ok(info);
                    }
                    _ => continue,
                }
            }
        }
        Err("Execution did not complete within polling timeout".into())
    }

    /// Fetch console output for a completed execution on the outer server.
    async fn get_console_output(
        &mut self,
        id: u64,
        execution_id: &str,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let msg = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": {
                "name": "get_execution_output",
                "arguments": { "execution_id": execution_id, "line_limit": 10000 }
            }
        });
        let response = self.send(msg).await?;
        Ok(common::extract_execution_info(&response)
            .and_then(|info| info["data"].as_str().map(String::from))
            .unwrap_or_default())
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
        names.contains(&"runjs__upstream__run_js".to_string()),
        "expected runjs__upstream__run_js in tool list, got: {:?}",
        names,
    );
    // Several upstream tools should be stubbed (run_js, get_execution, list_executions, ...).
    let stub_count = names.iter().filter(|n| n.starts_with("runjs__upstream__")).count();
    assert!(stub_count >= 2, "expected multiple upstream stubs, got: {:?}", names);

    // Stub schemas should mirror the upstream tool's schema. For run_js
    // upstream tool, the stub should describe a `code` parameter.
    let stub = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t.get("name").and_then(|n| n.as_str()) == Some("runjs__upstream__run_js"))
        .expect("stub present");
    let schema = &stub["inputSchema"];
    assert!(
        schema["properties"]["code"].is_object(),
        "stub schema should have `code` property; got {}",
        serde_json::to_string_pretty(schema).unwrap_or_default(),
    );
    let desc = stub["description"].as_str().unwrap_or_default();
    assert!(desc.contains("run_js"), "description: {}", desc);
    assert!(desc.contains("await mcp.upstream.run_js"), "description: {}", desc);

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
                "name": "runjs__upstream__run_js",
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
    assert!(text.contains("await mcp.upstream.run_js("), "stub call text: {}", text);
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
    assert!(!text.contains("This tool is a stub"), "native list_executions should not return stub text: {}", text);

    server.stop().await;
    common::cleanup_heap_dir(&outer_heap);
    common::cleanup_heap_dir(&upstream_heap);
    Ok(())
}

#[tokio::test]
async fn proxy_namespace_end_to_end() -> Result<(), Box<dyn std::error::Error>> {
    let outer_heap = common::create_temp_heap_dir() + "-outer-proxy";
    let upstream_heap = common::create_temp_heap_dir() + "-upstream-proxy";
    std::fs::create_dir_all(&outer_heap).ok();
    std::fs::create_dir_all(&upstream_heap).ok();

    let mut server = OuterServer::start(&outer_heap, &upstream_heap).await?;
    server.initialize().await?;

    // Exercise the whole JS surface in one execution: proxy dispatch with
    // result unwrapping, live catalog introspection, `in` / Object.keys
    // traps, migration traps for the removed v1 API, and unknown-tool errors.
    let code = r#"
        const r = await mcp.upstream.list_executions({});
        console.log('TYPE:' + (r === null ? 'null' : typeof r));
        console.log('RAW_ENVELOPE:' + ((r && r.content !== undefined && r.isError !== undefined) ? 'yes' : 'no'));
        console.log('TOOLS:' + JSON.stringify(mcp.tools('upstream').map(t => t.name)));
        console.log('HAS_RUNJS:' + ('run_js' in mcp.upstream));
        console.log('KEYS_EXACT:' + Object.keys(mcp.upstream).includes('run_js'));
        console.log('THEN_UNDEF:' + (mcp.upstream.then === undefined));
        console.log('TOSTRING:' + String(mcp.upstream).startsWith('[mcp server upstream'));
        const r2 = await mcp.upstream.list_executions({});
        console.log('REPEAT_CALL:' + (typeof r2 === 'object'));
        console.log('SERVERS:' + JSON.stringify(mcp.servers));
        try { mcp.callTool('upstream', 'run_js', {}); console.log('CALLTOOL_TRAP:none'); }
        catch (e) { console.log('CALLTOOL_TRAP:' + e.message); }
        try { mcp.listTools(); console.log('LISTTOOLS_TRAP:none'); }
        catch (e) { console.log('LISTTOOLS_TRAP:' + e.message); }
        try { await mcp.upstream.definitely_not_a_tool({}); console.log('UNKNOWN:none'); }
        catch (e) { console.log('UNKNOWN:' + e.message); }
    "#;

    let info = server.run_js_and_wait(2, code).await?;
    assert_eq!(
        info["status"].as_str(),
        Some("completed"),
        "execution should complete: {}",
        serde_json::to_string_pretty(&info).unwrap_or_default(),
    );
    let exec_id = info["execution_id"].as_str().unwrap().to_string();
    let output = server.get_console_output(3, &exec_id).await?;

    // Unwrap ladder: list_executions returns JSON text, so the proxy call
    // resolves to a parsed object — not the raw {content, isError} envelope.
    assert!(output.contains("TYPE:object"), "output: {}", output);
    assert!(output.contains("RAW_ENVELOPE:no"), "output: {}", output);
    // Live catalog introspection.
    assert!(output.contains("run_js"), "tools() should list run_js: {}", output);
    assert!(output.contains("HAS_RUNJS:true"), "output: {}", output);
    // Object.keys returns exact tool names (complete introspection: every
    // listed key is invokable via mcp[server][key]).
    assert!(output.contains("KEYS_EXACT:true"), "output: {}", output);
    // `then` never resolves so the namespace is not thenable, and toString
    // still introspects when no tool shadows it.
    assert!(output.contains("THEN_UNDEF:true"), "output: {}", output);
    assert!(output.contains("TOSTRING:true"), "output: {}", output);
    // Second dispatch exercises the generation-cached catalog path.
    assert!(output.contains("REPEAT_CALL:true"), "output: {}", output);
    assert!(output.contains("upstream"), "servers should list upstream: {}", output);
    // Migration traps.
    assert!(output.contains("CALLTOOL_TRAP:mcp.callTool() was removed"), "output: {}", output);
    assert!(output.contains("LISTTOOLS_TRAP:mcp.listTools() was replaced by mcp.tools("), "output: {}", output);
    // Unknown-tool errors name the missing tool and list what exists.
    assert!(output.contains("UNKNOWN:mcp.upstream has no tool \"definitely_not_a_tool\""), "output: {}", output);
    assert!(output.contains("Available tools:"), "output: {}", output);

    server.stop().await;
    common::cleanup_heap_dir(&outer_heap);
    common::cleanup_heap_dir(&upstream_heap);
    Ok(())
}

#[tokio::test]
async fn mcp_stub_prefix_flag_overrides_default() -> Result<(), Box<dyn std::error::Error>> {
    let outer_heap = common::create_temp_heap_dir() + "-outer3";
    let upstream_heap = common::create_temp_heap_dir() + "-upstream3";
    std::fs::create_dir_all(&outer_heap).ok();
    std::fs::create_dir_all(&upstream_heap).ok();

    let mut server = OuterServer::start_with_args(
        &outer_heap,
        &upstream_heap,
        &["--mcp-stub-prefix", "rj_"],
    )
    .await?;
    server.initialize().await?;

    let list = server
        .send(json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }))
        .await?;
    let names = tool_names(&list);
    // Stubs use the new prefix.
    assert!(
        names.contains(&"rj_upstream__run_js".to_string()),
        "expected rj_upstream__run_js in tool list, got: {:?}",
        names,
    );
    // The default prefix is no longer present.
    assert!(
        !names.iter().any(|n| n.starts_with("runjs__")),
        "default-prefixed stubs should not appear: {:?}",
        names,
    );

    server.stop().await;
    common::cleanup_heap_dir(&outer_heap);
    common::cleanup_heap_dir(&upstream_heap);
    Ok(())
}

#[tokio::test]
async fn mcp_stubs_disabled_flag_hides_stubs() -> Result<(), Box<dyn std::error::Error>> {
    let outer_heap = common::create_temp_heap_dir() + "-outer4";
    let upstream_heap = common::create_temp_heap_dir() + "-upstream4";
    std::fs::create_dir_all(&outer_heap).ok();
    std::fs::create_dir_all(&upstream_heap).ok();

    let mut server = OuterServer::start_with_args(
        &outer_heap,
        &upstream_heap,
        &["--mcp-stubs", "false"],
    )
    .await?;
    server.initialize().await?;

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
    // No stubs at all.
    assert!(
        !names.iter().any(|n| n.starts_with("runjs__")),
        "stubs should be hidden when --mcp-stubs false: {:?}",
        names,
    );

    // Calling a stub-shaped name now falls through to the normal dispatcher,
    // which returns "tool not found" rather than a stub instruction.
    let resp = server
        .send(json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "runjs__upstream__run_js",
                "arguments": {"code": "return 1;"}
            }
        }))
        .await?;
    let text = resp["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let err = resp["error"]["message"].as_str().unwrap_or_default().to_string();
    assert!(
        !text.contains("This tool is a stub"),
        "should not return stub instructions when stubs disabled: text={} err={}",
        text,
        err,
    );

    server.stop().await;
    common::cleanup_heap_dir(&outer_heap);
    common::cleanup_heap_dir(&upstream_heap);
    Ok(())
}
