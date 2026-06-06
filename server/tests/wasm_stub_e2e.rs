//! End-to-end test for WASM module stubbing on the MCP surface.
//!
//! Starts an MCPJS server over stdio with a pre-loaded WASM module
//! (`--wasm-module math=<fixtures>/add.wasm`). The server should advertise the
//! module as a `runjs__wasm__math` stub in `tools/list`, and calling that stub
//! should return instructional text telling the caller to use the module from
//! JavaScript via `run_js` (the module is exposed as the `__wasm_math` global).

use serde_json::{json, Value};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::time::timeout;

mod common;

struct WasmServer {
    child: Child,
    stdin: tokio::process::ChildStdin,
    stdout: BufReader<tokio::process::ChildStdout>,
}

impl WasmServer {
    /// Start an MCPJS server on stdio with `math=<fixtures>/add.wasm` loaded.
    /// `extra_args` lets a test pass flags like `--wasm-stubs false` or
    /// `--wasm-stub-prefix rj_`.
    async fn start_with_args(
        heap: &str,
        extra_args: &[&str],
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let server_bin = env!("CARGO_BIN_EXE_server");
        let wasm_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("add.wasm");

        let mut args: Vec<String> = vec![
            "--directory-path".to_string(),
            heap.to_string(),
            "--wasm-module".to_string(),
            format!("math={}", wasm_path.display()),
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

        tokio::time::sleep(Duration::from_millis(800)).await;

        Ok(Self { child, stdin, stdout })
    }

    async fn start(heap: &str) -> Result<Self, Box<dyn std::error::Error>> {
        Self::start_with_args(heap, &[]).await
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
                "clientInfo": {"name": "wasm-stub-e2e", "version": "1.0.0"}
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
async fn server_advertises_wasm_module_as_stub() -> Result<(), Box<dyn std::error::Error>> {
    let heap = common::create_temp_heap_dir() + "-wasm-stub1";
    std::fs::create_dir_all(&heap).ok();

    let mut server = WasmServer::start(&heap).await?;
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
    assert!(names.contains(&"run_js".to_string()), "native run_js missing: {:?}", names);
    assert!(
        names.contains(&"runjs__wasm__math".to_string()),
        "expected runjs__wasm__math stub, got: {:?}",
        names,
    );

    // The stub description should hint at run_js usage and the __wasm_ global.
    let stub = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t.get("name").and_then(|n| n.as_str()) == Some("runjs__wasm__math"))
        .expect("stub present");
    let desc = stub["description"].as_str().unwrap_or_default();
    assert!(desc.contains("run_js"), "description: {}", desc);
    assert!(desc.contains("__wasm_math"), "description: {}", desc);
    // add.wasm exports `add`.
    assert!(desc.contains("add"), "description should list exports: {}", desc);

    server.stop().await;
    common::cleanup_heap_dir(&heap);
    Ok(())
}

#[tokio::test]
async fn calling_wasm_stub_returns_run_js_instructions() -> Result<(), Box<dyn std::error::Error>> {
    let heap = common::create_temp_heap_dir() + "-wasm-stub2";
    std::fs::create_dir_all(&heap).ok();

    let mut server = WasmServer::start(&heap).await?;
    server.initialize().await?;

    let resp = server
        .send(json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "runjs__wasm__math",
                "arguments": {}
            }
        }))
        .await?;

    assert_eq!(resp["result"]["isError"], json!(false));
    let text = resp["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(text.contains("run_js"), "stub call text: {}", text);
    assert!(text.contains("math"), "stub call text: {}", text);

    server.stop().await;
    common::cleanup_heap_dir(&heap);
    Ok(())
}

#[tokio::test]
async fn wasm_stub_prefix_flag_overrides_default() -> Result<(), Box<dyn std::error::Error>> {
    let heap = common::create_temp_heap_dir() + "-wasm-stub3";
    std::fs::create_dir_all(&heap).ok();

    let mut server = WasmServer::start_with_args(&heap, &["--wasm-stub-prefix", "rj_"]).await?;
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
    assert!(
        names.contains(&"rj_wasm__math".to_string()),
        "expected rj_wasm__math stub, got: {:?}",
        names,
    );
    assert!(
        !names.iter().any(|n| n.starts_with("runjs__wasm__")),
        "default-prefixed wasm stubs should not appear: {:?}",
        names,
    );

    server.stop().await;
    common::cleanup_heap_dir(&heap);
    Ok(())
}

#[tokio::test]
async fn wasm_stubs_disabled_flag_hides_stubs() -> Result<(), Box<dyn std::error::Error>> {
    let heap = common::create_temp_heap_dir() + "-wasm-stub4";
    std::fs::create_dir_all(&heap).ok();

    let mut server = WasmServer::start_with_args(&heap, &["--wasm-stubs", "false"]).await?;
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
    assert!(names.contains(&"run_js".to_string()), "native run_js missing: {:?}", names);
    assert!(
        !names.iter().any(|n| n.starts_with("runjs__wasm__")),
        "wasm stubs should be hidden when --wasm-stubs false: {:?}",
        names,
    );

    // Calling the stub-shaped name now falls through to the normal dispatcher,
    // which does not return stub instructions.
    let resp = server
        .send(json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "runjs__wasm__math",
                "arguments": {}
            }
        }))
        .await?;
    let text = resp["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !text.contains("Execute it from"),
        "should not return wasm stub instructions when disabled: {}",
        text,
    );

    server.stop().await;
    common::cleanup_heap_dir(&heap);
    Ok(())
}
