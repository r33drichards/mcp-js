/// Common utilities for integration tests

use std::time::Duration;
use tokio::process::Command;
use tokio::time::sleep;

/// Start the MCP server for testing
pub async fn start_server(port: u16, heap_dir: &str) -> Result<tokio::process::Child, std::io::Error> {
    let child = Command::new(env!("CARGO"))
        .args(&["run", "--", "--directory-path", heap_dir, "--http-port", &port.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;

    // Give server time to start
    sleep(Duration::from_millis(500)).await;

    Ok(child)
}

/// Start the MCP server with SSE transport for testing
pub async fn start_sse_server(port: u16, heap_dir: &str) -> Result<tokio::process::Child, std::io::Error> {
    let child = Command::new(env!("CARGO"))
        .args(&["run", "--", "--directory-path", heap_dir, "--sse-port", &port.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;

    // Give server time to start
    sleep(Duration::from_millis(500)).await;

    Ok(child)
}

/// Stop the MCP server
pub async fn stop_server(mut child: tokio::process::Child) {
    let _ = child.kill().await;
}

/// Create a temporary directory for heap storage
pub fn create_temp_heap_dir() -> String {
    let temp_dir = std::env::temp_dir();
    let heap_dir = temp_dir.join(format!("mcp-test-heap-{}", std::process::id()));
    std::fs::create_dir_all(&heap_dir).ok();
    heap_dir.to_string_lossy().to_string()
}

/// Clean up temporary heap directory
pub fn cleanup_heap_dir(dir: &str) {
    let _ = std::fs::remove_dir_all(dir);
}

/// Extract the content-addressed heap hash from a JSON-RPC tool call response.
/// The response format is: result.content[0].text = '{"output":"...","heap":"<hash>"}'
pub fn extract_heap_hash(response: &serde_json::Value) -> Option<String> {
    let text = response["result"]["content"]
        .as_array()?
        .first()?
        .get("text")?
        .as_str()?;
    let parsed: serde_json::Value = serde_json::from_str(text).ok()?;
    parsed["heap"].as_str().map(|s| s.to_string())
}

/// Extract the execution_id from a run_js response.
/// The response format is: result.content[0].text = '{"execution_id":"<id>"}'
pub fn extract_execution_id(response: &serde_json::Value) -> Option<String> {
    let text = response["result"]["content"]
        .as_array()?
        .first()?
        .get("text")?
        .as_str()?;
    let parsed: serde_json::Value = serde_json::from_str(text).ok()?;
    parsed["execution_id"].as_str().map(|s| s.to_string())
}

/// Extract fields from a get_execution response.
/// The response format is: result.content[0].text = '{"execution_id":"...","status":"...","result":"...","heap":"...",...}'
pub fn extract_execution_info(response: &serde_json::Value) -> Option<serde_json::Value> {
    let text = response["result"]["content"]
        .as_array()?
        .first()?
        .get("text")?
        .as_str()?;
    serde_json::from_str(text).ok()
}
