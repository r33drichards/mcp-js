/// End-to-end tests for single-file configuration (`--config`).
///
/// Each test spawns the real server binary with a config file and observes
/// which HTTP port actually serves, since the port is the easiest
/// externally-visible effect of a config value. Precedence under test:
/// explicit CLI flag > MCP_V8_* env var > config file > built-in default.

use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::net::TcpListener;
use tokio::process::{Child, Command};

/// Reserve a free localhost port (released before the server binds it).
async fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind to an ephemeral port");
    listener.local_addr().unwrap().port()
}

fn temp_dir(tag: &str) -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix(&format!("mcp-config-e2e-{tag}-"))
        .tempdir()
        .expect("create temp dir")
}

fn write_config(dir: &tempfile::TempDir, name: &str, contents: &str) -> String {
    let path = dir.path().join(name);
    std::fs::write(&path, contents).expect("write config file");
    path.to_string_lossy().to_string()
}

fn spawn_server(args: &[&str], envs: &[(&str, &str)]) -> Child {
    let mut command = Command::new(env!("CARGO_BIN_EXE_server"));
    command.args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    for (key, value) in envs {
        command.env(key, value);
    }
    command.kill_on_drop(true).spawn().expect("spawn server binary")
}

/// Poll the OpenAPI endpoint until the server answers on `port`.
async fn wait_for_http(port: u16) {
    let url = format!("http://127.0.0.1:{port}/api-doc/openapi.json");
    let client = reqwest::Client::new();
    for _ in 0..100 {
        if let Ok(response) = client.get(&url).send().await {
            if response.status().is_success() {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("server did not start serving HTTP on port {port}");
}

async fn refuses_connections(port: u16) -> bool {
    tokio::net::TcpStream::connect(("127.0.0.1", port)).await.is_err()
}

fn base_config(dir: &tempfile::TempDir, port: u16) -> String {
    format!(
        "http_port = {port}\nsession_db_path = \"{}\"\n",
        dir.path().join("sessions").display()
    )
}

#[tokio::test]
async fn toml_config_file_configures_the_server() {
    let dir = temp_dir("toml");
    let port = free_port().await;
    // Kebab-case key and a heap axis setting exercise more than the port.
    let config = format!(
        "{}heap-store = \"dir\"\nheap_dir = \"{}\"\n",
        base_config(&dir, port),
        dir.path().join("heaps").display()
    );
    let config_path = write_config(&dir, "server.toml", &config);

    let mut child = spawn_server(&["--config", &config_path], &[]);
    wait_for_http(port).await;
    let _ = child.kill().await;
}

#[tokio::test]
async fn json_config_file_configures_the_server() {
    let dir = temp_dir("json");
    let port = free_port().await;
    let config = serde_json::json!({
        "http_port": port,
        "session_db_path": dir.path().join("sessions").to_string_lossy(),
    });
    let config_path = write_config(&dir, "server.json", &config.to_string());

    let mut child = spawn_server(&["--config", &config_path], &[]);
    wait_for_http(port).await;
    let _ = child.kill().await;
}

#[tokio::test]
async fn cli_flag_beats_config_file() {
    let dir = temp_dir("cli-wins");
    let config_port = free_port().await;
    let cli_port = free_port().await;
    let config_path = write_config(&dir, "server.toml", &base_config(&dir, config_port));

    let mut child =
        spawn_server(&["--config", &config_path, "--http-port", &cli_port.to_string()], &[]);
    wait_for_http(cli_port).await;
    assert!(
        refuses_connections(config_port).await,
        "the config-file port must not be bound when the CLI overrides it"
    );
    let _ = child.kill().await;
}

#[tokio::test]
async fn env_var_beats_config_file() {
    let dir = temp_dir("env-wins");
    let config_port = free_port().await;
    let env_port = free_port().await;
    let config_path = write_config(&dir, "server.toml", &base_config(&dir, config_port));

    let mut child = spawn_server(
        &["--config", &config_path],
        &[("MCP_V8_HTTP_PORT", &env_port.to_string())],
    );
    wait_for_http(env_port).await;
    assert!(
        refuses_connections(config_port).await,
        "the config-file port must not be bound when the env var overrides it"
    );
    let _ = child.kill().await;
}

#[tokio::test]
async fn unknown_config_key_fails_startup() {
    let dir = temp_dir("unknown-key");
    let config_path = write_config(&dir, "server.toml", "htpp_port = 8080\n");

    let mut child = Command::new(env!("CARGO_BIN_EXE_server"))
        .args(["--config", &config_path])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn server binary");

    let mut stderr = String::new();
    child.stderr.take().unwrap().read_to_string(&mut stderr).await.unwrap();
    let status = child.wait().await.expect("server should exit on its own");

    assert!(!status.success(), "a typo'd config key must fail startup");
    assert!(stderr.contains("unknown key"), "stderr should name the problem: {stderr}");
    assert!(stderr.contains("htpp_port"), "stderr should echo the bad key: {stderr}");
    assert!(stderr.contains("http_port"), "stderr should list accepted keys: {stderr}");
}
