/// Integration tests for the /api/version and /api/cli/* endpoints.
///
/// Tests cover:
///   GET /api/version              → returns the server version string
///   GET /api/cli                  → returns index with platform list, version, and available flags
///   GET /api/cli/{platform}       → streams the embedded binary (or 404 if not embedded)
///   GET /api/cli/{unknown}        → 404 with error message listing valid platforms
///
/// The binary-serving path is only exercised when the server was built with
/// MCP_V8_CLI_LINUX_X86_64 set (i.e. in release CI). In dev builds the
/// endpoint correctly returns 404 with an informative message, and that
/// behaviour is also tested here.

use reqwest::Client;
use std::process::Stdio;
use tokio::process::Command;
use tokio::time::{sleep, timeout, Duration};

// ── Server helper ────────────────────────────────────────────────────────

struct HttpServer {
    child: Option<tokio::process::Child>,
    pub base_url: String,
}

impl HttpServer {
    async fn start() -> Result<Self, Box<dyn std::error::Error>> {
        let port = find_available_port();

        let child = Command::new(env!("CARGO_BIN_EXE_server"))
            .args(["--stateless", "--http-port", &port.to_string()])
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

// ── /api/version ─────────────────────────────────────────────────────────

/// GET /api/version returns a JSON object with a non-empty version string.
#[tokio::test]
async fn test_version_returns_version_string() {
    let mut server = HttpServer::start().await.expect("server start");
    let client = Client::new();

    let resp = client
        .get(format!("{}/api/version", server.base_url))
        .send()
        .await
        .expect("GET /api/version");

    assert_eq!(resp.status(), 200, "expected 200");

    let body: serde_json::Value = resp.json().await.expect("parse JSON");
    let version = body["version"].as_str().expect("version field missing");
    assert!(!version.is_empty(), "version should not be empty");

    // Must be semver-ish: at least one dot
    assert!(version.contains('.'), "version '{version}' does not look like semver");

    server.stop().await;
}

/// The version returned by /api/version matches CARGO_PKG_VERSION.
#[tokio::test]
async fn test_version_matches_cargo_pkg_version() {
    let mut server = HttpServer::start().await.expect("server start");
    let client = Client::new();

    let body: serde_json::Value = client
        .get(format!("{}/api/version", server.base_url))
        .send()
        .await
        .expect("GET /api/version")
        .json()
        .await
        .expect("parse JSON");

    let got = body["version"].as_str().expect("version field");
    let expected = env!("CARGO_PKG_VERSION");
    assert_eq!(got, expected, "version mismatch: got {got}, want {expected}");

    server.stop().await;
}

// ── /api/cli (index) ──────────────────────────────────────────────────────

/// GET /api/cli returns a JSON object with version and an assets array.
#[tokio::test]
async fn test_cli_index_shape() {
    let mut server = HttpServer::start().await.expect("server start");
    let client = Client::new();

    let resp = client
        .get(format!("{}/api/cli", server.base_url))
        .send()
        .await
        .expect("GET /api/cli");

    assert_eq!(resp.status(), 200);

    let body: serde_json::Value = resp.json().await.expect("parse JSON");

    // Top-level shape
    assert!(body["version"].is_string(), "missing version");
    assert!(body["assets"].is_array(), "missing assets");

    let assets = body["assets"].as_array().unwrap();
    assert!(!assets.is_empty(), "assets should not be empty");

    // Every asset must have platform, url, and available fields
    for asset in assets {
        let platform = asset["platform"].as_str().expect("asset missing platform");
        assert!(!platform.is_empty());
        assert!(asset["url"].is_string(), "asset {platform} missing url");
        assert!(asset["available"].is_boolean(), "asset {platform} missing available");
    }

    server.stop().await;
}

/// The index covers all four expected platforms.
#[tokio::test]
async fn test_cli_index_covers_all_platforms() {
    let mut server = HttpServer::start().await.expect("server start");
    let client = Client::new();

    let body: serde_json::Value = client
        .get(format!("{}/api/cli", server.base_url))
        .send()
        .await
        .expect("GET /api/cli")
        .json()
        .await
        .expect("parse JSON");

    let platforms: Vec<&str> = body["assets"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|a| a["platform"].as_str())
        .collect();

    for expected in &["linux-x86_64", "linux-aarch64", "macos-aarch64"] {
        assert!(platforms.contains(expected), "missing platform {expected}");
    }

    server.stop().await;
}

/// The version in /api/cli matches /api/version.
#[tokio::test]
async fn test_cli_index_version_matches_version_endpoint() {
    let mut server = HttpServer::start().await.expect("server start");
    let client = Client::new();

    let version_body: serde_json::Value = client
        .get(format!("{}/api/version", server.base_url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let cli_body: serde_json::Value = client
        .get(format!("{}/api/cli", server.base_url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(
        version_body["version"],
        cli_body["version"],
        "/api/cli version should match /api/version"
    );

    server.stop().await;
}

/// The url field in each asset points back to this server's /api/cli/{platform}.
#[tokio::test]
async fn test_cli_index_urls_point_to_this_server() {
    let mut server = HttpServer::start().await.expect("server start");
    let client = Client::new();

    let body: serde_json::Value = client
        .get(format!("{}/api/cli", server.base_url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    for asset in body["assets"].as_array().unwrap() {
        let url = asset["url"].as_str().unwrap();
        let platform = asset["platform"].as_str().unwrap();
        assert!(
            url.starts_with(&server.base_url),
            "url '{url}' should start with server base '{}'", server.base_url
        );
        assert!(
            url.ends_with(&format!("/api/cli/{}", platform)),
            "url '{url}' should end with /api/cli/{platform}"
        );
    }

    server.stop().await;
}

// ── /api/cli/{platform} ───────────────────────────────────────────────────

/// GET /api/cli/{unknown-platform} returns 404 with an error JSON body.
#[tokio::test]
async fn test_cli_download_unknown_platform_returns_404() {
    let mut server = HttpServer::start().await.expect("server start");
    let client = Client::new();

    let resp = client
        .get(format!("{}/api/cli/windows-x86_64", server.base_url))
        .send()
        .await
        .expect("GET /api/cli/windows-x86_64");

    assert_eq!(resp.status(), 404);

    let body: serde_json::Value = resp.json().await.expect("parse JSON");
    let error = body["error"].as_str().expect("error field missing");
    assert!(
        error.contains("windows-x86_64"),
        "error should mention the requested platform"
    );
    // Should list valid platforms
    assert!(error.contains("linux-x86_64"), "error should list valid platforms");

    server.stop().await;
}

/// GET /api/cli/{platform} for a known platform either:
///   - returns 200 with binary content (release builds with embedded CLI), or
///   - returns 404 with a helpful "not embedded" message (dev builds).
/// Either outcome is acceptable; we assert the correct shape for each case.
#[tokio::test]
async fn test_cli_download_known_platform_returns_binary_or_helpful_404() {
    let mut server = HttpServer::start().await.expect("server start");
    let client = Client::new();

    let resp = client
        .get(format!("{}/api/cli/linux-x86_64", server.base_url))
        .send()
        .await
        .expect("GET /api/cli/linux-x86_64");

    match resp.status().as_u16() {
        200 => {
            // Release build: should be a real binary
            let ct = resp
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            assert!(
                ct.contains("octet-stream"),
                "expected octet-stream content-type, got '{ct}'"
            );

            let cd = resp
                .headers()
                .get("content-disposition")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            assert!(
                cd.contains("mcp-v8-cli"),
                "content-disposition should include filename, got '{cd}'"
            );

            let bytes = resp.bytes().await.expect("read body");
            assert!(!bytes.is_empty(), "binary should not be empty");

            // ELF magic bytes for Linux binary: 0x7f 'E' 'L' 'F'
            assert_eq!(&bytes[..4], b"\x7fELF", "expected ELF binary");
        }
        404 => {
            // Dev build: embedded binary not present
            let body: serde_json::Value = resp.json().await.expect("parse 404 JSON");
            let error = body["error"].as_str().expect("error field");
            assert!(
                error.contains("not embedded") || error.contains("MCP_V8_CLI"),
                "404 error should explain how to embed: '{error}'"
            );
        }
        other => panic!("unexpected status {other}"),
    }

    server.stop().await;
}

/// The available flag in the index is consistent with the download endpoint.
#[tokio::test]
async fn test_cli_available_flag_consistent_with_download() {
    let mut server = HttpServer::start().await.expect("server start");
    let client = Client::new();

    let index: serde_json::Value = client
        .get(format!("{}/api/cli", server.base_url))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    for asset in index["assets"].as_array().unwrap() {
        let platform = asset["platform"].as_str().unwrap();
        let available = asset["available"].as_bool().unwrap();

        let resp = client
            .get(format!("{}/api/cli/{}", server.base_url, platform))
            .send()
            .await
            .unwrap();

        if available {
            assert_eq!(
                resp.status(), 200,
                "platform '{platform}' is marked available but download returned {}",
                resp.status()
            );
        } else {
            assert_eq!(
                resp.status(), 404,
                "platform '{platform}' is marked unavailable but download returned {}",
                resp.status()
            );
        }
    }

    server.stop().await;
}

// ── Full E2E: download CLI from API, run it, check output ─────────────────

/// Download the CLI binary from /api/cli/linux-x86_64, write it to a temp
/// file, make it executable, then run:
///
///   mcp-v8-cli --url <server> --json exec 'console.log(1+1)'
///
/// Poll until the execution is complete, read the output, and assert it
/// contains "2".
///
/// This test is skipped (with a clear message) when the CLI binary is not
/// embedded in the server build (dev/local builds). It is the definitive
/// end-to-end test for the feature: the same binary that ships in the server
/// is the one used to drive it.
#[tokio::test]
async fn test_downloaded_cli_exec_console_log_outputs_2() {
    let mut server = HttpServer::start().await.expect("server start");
    let client = Client::new();

    // ── Step 1: download the CLI from the server ──────────────────────
    let resp = client
        .get(format!("{}/api/cli/linux-x86_64", server.base_url))
        .send()
        .await
        .expect("GET /api/cli/linux-x86_64");

    if resp.status() == 404 {
        // Dev build — binary not embedded; skip gracefully.
        eprintln!(
            "[SKIP] test_downloaded_cli_exec_console_log_outputs_2: \
             CLI not embedded in this build (set MCP_V8_CLI_LINUX_X86_64 at build time)"
        );
        server.stop().await;
        return;
    }

    assert_eq!(resp.status(), 200, "expected 200 downloading CLI");

    let cli_bytes = resp.bytes().await.expect("read CLI binary");
    assert!(!cli_bytes.is_empty(), "downloaded CLI is empty");

    // ── Step 2: write to a temp file and make executable ─────────────
    let cli_path = std::env::temp_dir().join(format!(
        "mcp-v8-cli-e2e-{}",
        std::process::id()
    ));
    std::fs::write(&cli_path, &cli_bytes).expect("write CLI binary");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&cli_path, std::fs::Permissions::from_mode(0o755))
            .expect("chmod +x");
    }

    // ── Step 3: submit 'console.log(1+1)' via the downloaded CLI ─────
    let output = timeout(
        Duration::from_secs(30),
        Command::new(&cli_path)
            .args([
                "--url", &server.base_url,
                "--json",
                "exec",
                "console.log(1+1)",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output(),
    )
    .await
    .expect("exec timed out")
    .expect("spawn CLI");

    assert!(
        output.status.success(),
        "CLI exec failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let exec_json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("CLI --json output is not valid JSON");
    let exec_id = exec_json["execution_id"]
        .as_str()
        .expect("execution_id missing from CLI output");

    // ── Step 4: poll via the downloaded CLI until completed ───────────
    let mut status = String::new();
    for _ in 0..60 {
        let poll = Command::new(&cli_path)
            .args([
                "--url", &server.base_url,
                "--json",
                "executions", "get", exec_id,
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .expect("CLI executions get");

        let info: serde_json::Value =
            serde_json::from_slice(&poll.stdout).expect("poll output not JSON");
        status = info["status"].as_str().unwrap_or("").to_string();

        if matches!(status.as_str(), "completed" | "failed" | "timed_out" | "cancelled") {
            break;
        }
        sleep(Duration::from_millis(200)).await;
    }

    assert_eq!(status, "completed", "execution did not complete: status={status}");

    // ── Step 5: read console output via the downloaded CLI ────────────
    let output_cmd = Command::new(&cli_path)
        .args([
            "--url", &server.base_url,
            "executions", "output", exec_id,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .expect("CLI executions output");

    let stdout = String::from_utf8_lossy(&output_cmd.stdout);
    assert!(
        stdout.trim().contains("2"),
        "expected output to contain '2', got: {stdout:?}"
    );

    // ── Cleanup ───────────────────────────────────────────────────────
    let _ = std::fs::remove_file(&cli_path);
    server.stop().await;
}
