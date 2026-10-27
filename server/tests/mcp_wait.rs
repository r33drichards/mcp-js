//! Request-scoped hosts must not freeze V8 before snapshot persistence finishes.
use serde_json::{Value, json};
use server::engine::ffi_config::{
    BlobStoreBuilder, EngineConfigBuilder, ExecutionLimitsBuilder, FilesystemAccessBuilder,
    StoreBackend,
};
use server::engine::{Engine, ToolCallRequest, initialize_v8};
use std::{path::Path, sync::Arc};

fn engine(dir: &Path) -> Arc<Engine> {
    let policy = dir.join("fs.rego");
    std::fs::write(&policy, "package mcp.filesystem\ndefault allow = false\nallow if { startswith(input.path, \"/work/\") }\n").unwrap();
    Engine::create(
        EngineConfigBuilder::new()
            .limits(
                ExecutionLimitsBuilder::new()
                    .heap_memory_max_mb(64)
                    .execution_timeout_secs(2)
                    .build()
                    .unwrap(),
            )
            .data_dir(dir.to_string_lossy().to_string())
            .filesystem(
                FilesystemAccessBuilder::new()
                    .policies_json(
                        json!({"policies":[{"url":format!("file://{}",policy.display())}]})
                            .to_string(),
                    )
                    .build()
                    .unwrap(),
            )
            .fs_snapshot_store(
                BlobStoreBuilder::new()
                    .backend(StoreBackend::Directory)
                    .build()
                    .unwrap(),
            )
            .build()
            .unwrap(),
    )
    .unwrap()
}

async fn call(engine: &Arc<Engine>, args: Value) -> Value {
    engine
        .invoke_tool_response(ToolCallRequest {
            name: "run_js".into(),
            arguments_json: args.to_string(),
            session_id: Some("wait-proof".into()),
            mcp_headers: None,
        })
        .await
        .unwrap()
        .json
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wait_settles_execution_and_filesystem_before_reply() {
    initialize_v8();
    let dir = tempfile::tempdir().unwrap();
    let first = engine(dir.path());
    let written = call(&first, json!({"wait":true,"code":"await new Promise(r => setTimeout(r, 50)); await fs.writeFile('/work/proof.txt', 'persisted'); console.log('written');"})).await;
    assert_eq!(written["status"], "completed", "{written}");
    assert!(written["output"].as_str().unwrap().contains("written"));
    assert_eq!(written["output_truncated"], false);
    assert_eq!(written["fs"].as_str().unwrap().len(), 64);
    // The session pointer must be durable before the caller freezes/exits.
    first.shutdown().await;
    drop(first);
    let reopened = engine(dir.path());
    let read = call(
        &reopened,
        json!({"wait":true,"code":"console.log(await fs.readFile('/work/proof.txt', 'utf8'));"}),
    )
    .await;
    assert_eq!(read["status"], "completed", "{read}");
    assert!(read["output"].as_str().unwrap().contains("persisted"));
    let output = reopened
        .get_execution_output(
            read["execution_id"].as_str().unwrap().into(),
            None,
            None,
            None,
            None,
        )
        .unwrap();
    assert!(output.data.contains("persisted"));
    let failed = call(
        &reopened,
        json!({"wait":true,"code":"throw new Error('expected failure');"}),
    )
    .await;
    assert_eq!(failed["status"], "failed", "{failed}");
    assert!(
        failed["error"]
            .as_str()
            .unwrap()
            .contains("expected failure")
    );
    let rejected = call(&reopened, json!({"wait":true,"file":"/work/proof.txt"})).await;
    assert!(
        rejected["error"]
            .as_str()
            .unwrap()
            .contains("file-path execution is disabled"),
        "{rejected}"
    );
    assert!(rejected.get("execution_id").is_none());
    // Omission keeps the old execution-id-only async response contract.
    let asynchronous = call(
        &reopened,
        json!({"code":"await new Promise(r => setTimeout(r, 100)); console.log('async');"}),
    )
    .await;
    assert!(asynchronous.get("status").is_none(), "{asynchronous}");
    reopened
        .clone()
        .await_execution(asynchronous["execution_id"].as_str().unwrap().into())
        .await
        .unwrap();
    reopened.shutdown().await;
}

#[tokio::test]
async fn explicit_wait_stays_inline_for_task_capable_http_clients() {
    let dir = tempfile::tempdir().unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_server"))
        .args([
            "--http-port",
            &port.to_string(),
            "--fs-store",
            "dir",
            "--session-id",
            "wire-proof",
            "--session-db-path",
            dir.path().to_str().unwrap(),
        ])
        .kill_on_drop(true)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap();
    let mut ready = false;
    for _ in 0..100 {
        if client
            .get(format!("{base}/api/version"))
            .send()
            .await
            .is_ok()
        {
            ready = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(ready);
    let response = client.post(format!("{base}/mcp"))
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "tools/call").header("Mcp-Name", "run_js")
        .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{
            "name":"run_js","arguments":{"code":"console.log(42)","wait":true},
            "_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28",
                "io.modelcontextprotocol/clientCapabilities":{"extensions":{"io.modelcontextprotocol/tasks":{}}}}
        }})).send().await.unwrap();
    assert!(response.status().is_success());
    let body = response.text().await.unwrap();
    let raw = body
        .lines()
        .find_map(|line| line.strip_prefix("data: "))
        .unwrap_or(&body);
    let rpc: Value = serde_json::from_str(raw).unwrap();
    assert_eq!(rpc["result"]["resultType"], "complete", "{rpc}");
    let result: Value =
        serde_json::from_str(rpc["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(result["status"], "completed", "{result}");
    assert!(result["output"].as_str().unwrap().contains("42"));
    child.kill().await.unwrap();
    child.wait().await.unwrap();
}
