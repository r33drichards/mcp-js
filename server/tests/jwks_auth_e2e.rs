//! End-to-end tests for bearer-token enforcement (`--jwks-url`) on the
//! Streamable HTTP transport, covering session-less requests.
//!
//! When a JWKS URL is configured, `enforce_bearer_auth` sits in front of
//! `/mcp` and the REST API and rejects any request without a JWKS-verified
//! bearer token with `401 Unauthorized` — before the MCP service (and its
//! header capture) runs. Session-less per-request-negotiated POSTs
//! (protocol `2026-07-28`) never send `initialize`, so this pins the
//! fail-closed behavior for them specifically: no token and bad token are
//! rejected, a valid token executes.

use jsonwebtoken::{EncodingKey, Header, encode};
use reqwest::Client;
use serde_json::{Value, json};
use std::process::Stdio;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::time::{Duration, sleep};

const PROTOCOL_VERSION: &str = "2026-07-28";
const HMAC_SECRET: &[u8] = b"jwks-auth-e2e-shared-secret-0123456789abcdef";
const KEY_ID: &str = "jwks-auth-e2e-key";

// ── Minimal JWKS endpoint ──────────────────────────────────────────────────
//
// Serves one symmetric (oct / HS256) JWK over hand-rolled HTTP/1.1 — enough
// for the server's `JwksKeyStore` (a plain reqwest GET) without pulling a
// web framework into dev-dependencies.

fn jwks_body() -> String {
    use base64::Engine as _;
    let k = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(HMAC_SECRET);
    json!({
        "keys": [{
            "kty": "oct",
            "kid": KEY_ID,
            "alg": "HS256",
            "k": k,
        }]
    })
    .to_string()
}

async fn start_jwks_server() -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind jwks listener");
    let url = format!("http://{}/jwks.json", listener.local_addr().unwrap());
    let body = jwks_body();
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let body = body.clone();
            tokio::spawn(async move {
                // Drain the request head; the path doesn't matter.
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf).await;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.shutdown().await;
            });
        }
    });
    (url, task)
}

/// Mint an HS256 token the JWKS above verifies.
fn valid_token() -> String {
    let mut header = Header::new(jsonwebtoken::Algorithm::HS256);
    header.kid = Some(KEY_ID.to_string());
    let exp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 3600;
    let claims = json!({ "sub": "jwks-auth-e2e", "exp": exp });
    encode(&header, &claims, &EncodingKey::from_secret(HMAC_SECRET)).expect("encode jwt")
}

// ── Server harness ─────────────────────────────────────────────────────────

struct HttpServer {
    child: Option<tokio::process::Child>,
    base_url: String,
    _data: tempfile::TempDir,
}

impl HttpServer {
    async fn start(jwks_url: &str) -> Result<Self, Box<dyn std::error::Error>> {
        Self::start_mode(jwks_url, false).await
    }

    async fn start_mode(
        jwks_url: &str,
        scheduled: bool,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let port = find_available_port();
        let data = tempfile::tempdir()?;
        let mut command = Command::new(env!("CARGO_BIN_EXE_server"));
        command
            .args(["--http-port", &port.to_string(), "--jwks-url", jwks_url])
            .env("MCP_V8_HTTP_FUNCTIONS_DIR", data.path().join("functions"))
            .args(["--heap-store", "none", "--session-db-path"])
            .arg(data.path().join("sessions"))
            .kill_on_drop(true)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if scheduled {
            let config = data.path().join("worker.json");
            std::fs::write(&config, json!({"namespace":"test", "worker_id":"test-worker",
                "data_dir":data.path().join("worker"), "bearer_env":"HTTP_FUNCTION_TEST_WORKER_TOKEN",
                "admission_bearer_env":"HTTP_FUNCTION_TEST_ADMISSION_TOKEN",
                "admission_urls":["http://127.0.0.1:1"]}).to_string())?;
            command
                .env("MCP_SCHEDULED_WORKER_CONFIG", config)
                .env("HTTP_FUNCTION_TEST_WORKER_TOKEN", "test-worker-token")
                .env("HTTP_FUNCTION_TEST_ADMISSION_TOKEN", "test-admission-token");
        }
        let child = command.spawn()?;

        let base_url = format!("http://127.0.0.1:{}", port);
        let client = Client::new();
        // The REST API sits behind auth too: readiness is "the socket
        // answers", whatever the status code.
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
                    _data: data,
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

// ── Session-less request helper ────────────────────────────────────────────

fn sessionless_run_js_body() -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "run_js",
            "arguments": { "code": "console.log(6 * 7)" },
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": PROTOCOL_VERSION,
                "io.modelcontextprotocol/clientInfo": { "name": "jwks-auth-e2e", "version": "1.0.0" },
                "io.modelcontextprotocol/clientCapabilities": {}
            }
        }
    })
}

fn sessionless_request(client: &Client, url: &str, token: Option<&str>) -> reqwest::RequestBuilder {
    management_request(client, url, token, "run_js", sessionless_run_js_body())
}

fn management_request(
    client: &Client,
    url: &str,
    token: Option<&str>,
    name: &str,
    body: Value,
) -> reqwest::RequestBuilder {
    let mut req = client
        .post(url)
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", PROTOCOL_VERSION)
        .header("Mcp-Method", "tools/call")
        .header("Mcp-Name", name)
        .json(&body);
    if let Some(token) = token {
        req = req.header("Authorization", format!("Bearer {token}"));
    }
    req
}

// ── Tests ──────────────────────────────────────────────────────────────────

/// With JWKS enforcement on, a session-less POST fails closed: no token and
/// an unverifiable token are both rejected with 401 before any execution,
/// while a JWKS-verified token executes normally.
#[tokio::test]
async fn sessionless_requests_fail_closed_without_valid_token() {
    let (jwks_url, jwks_task) = start_jwks_server().await;
    let mut server = HttpServer::start(&jwks_url).await.expect("server start");
    let client = Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("client");
    let url = server.mcp_url();

    // No token → 401, no JSON-RPC result.
    let resp = sessionless_request(&client, &url, None)
        .send()
        .await
        .expect("request without token");
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "missing token must be rejected"
    );
    let body = resp.text().await.unwrap_or_default();
    assert!(
        !body.contains("\"result\""),
        "rejected request must not carry a tool result: {body}"
    );

    // Garbage token → 401.
    let resp = sessionless_request(&client, &url, Some("not.a.jwt"))
        .send()
        .await
        .expect("request with invalid token");
    assert_eq!(
        resp.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "unverifiable token must be rejected"
    );

    // Valid HS256 token (verified via the JWKS endpoint) → executes.
    let resp = sessionless_request(&client, &url, Some(&valid_token()))
        .send()
        .await
        .expect("request with valid token");
    assert!(
        resp.status().is_success(),
        "valid token should be accepted: {}",
        resp.status()
    );
    let body = resp.text().await.expect("body");
    assert!(
        body.contains("42"),
        "authorized call should carry run_js output 42: {body}"
    );

    let definition_url = format!("{}/api/http-functions/v1/definitions/echo", server.base_url);
    let invoke_url = format!("{}/functions/v1/echo", server.base_url);
    let definition = json!({"version":1,"methods":["POST"],"export":"handler",
        "source":"async function handler(r) { return {status:200,body:r.method}; }"});
    for token in [None, Some("not.a.jwt")] {
        for (method, endpoint) in [
            ("PUT", &definition_url),
            ("GET", &definition_url),
            ("DELETE", &definition_url),
            ("POST", &invoke_url),
        ] {
            let mut request = client
                .request(method.parse().unwrap(), endpoint)
                .json(&definition);
            if let Some(token) = token {
                request = request.bearer_auth(token);
            }
            assert_eq!(
                request.send().await.unwrap().status().as_u16(),
                401,
                "{method} {endpoint}"
            );
        }
    }
    let token = valid_token();
    let registered = client
        .put(&definition_url)
        .bearer_auth(&token)
        .header("if-none-match", "*")
        .json(&definition)
        .send()
        .await
        .unwrap();
    assert_eq!(registered.status().as_u16(), 200);
    let etag = registered.headers()["etag"].clone();
    let invoked = client
        .post(&invoke_url)
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(invoked.status().as_u16(), 200);
    assert_eq!(invoked.text().await.unwrap(), "POST");
    assert_eq!(
        client
            .delete(&definition_url)
            .bearer_auth(&token)
            .header("if-match", etag)
            .send()
            .await
            .unwrap()
            .status()
            .as_u16(),
        200
    );

    server.stop().await;
    jwks_task.abort();
}

#[tokio::test]
async fn scheduled_only_transport_does_not_mount_function_or_legacy_routes() {
    let (jwks_url, jwks_task) = start_jwks_server().await;
    let mut server = HttpServer::start_mode(&jwks_url, true).await.unwrap();
    let client = Client::new();
    let token = valid_token();
    for (method, path) in [
        ("POST", "/functions/v1/echo"),
        ("PUT", "/api/http-functions/v1/definitions/echo"),
        ("GET", "/api/http-functions/v1/definitions"),
        ("POST", "/api/exec"),
        ("POST", "/mcp"),
    ] {
        let reply = client
            .request(
                method.parse().unwrap(),
                format!("{}{path}", server.base_url),
            )
            .bearer_auth(&token)
            .send()
            .await
            .unwrap();
        assert_eq!(reply.status().as_u16(), 404, "{method} {path}");
    }
    server.stop().await;
    jwks_task.abort();
}

#[tokio::test]
async fn function_mcp_management_requires_transport_authorization_for_every_tool() {
    let (jwks_url, jwks_task) = start_jwks_server().await;
    let mut server = HttpServer::start(&jwks_url).await.unwrap();
    let client = Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap();
    let definition = json!({"version":1,"methods":["POST"],"export":"handler",
        "source":"function handler(){return {status:200,body:'authorized'}}",
        "limits":{"timeout_seconds":3,"heap_mb":32}});
    for (name, arguments) in [
        (
            "http_function_create",
            json!({"name":"protected","definition":definition}),
        ),
        ("http_function_list", json!({})),
        ("http_function_lookup", json!({"name":"protected"})),
        (
            "http_function_update",
            json!({"name":"protected","revision":"00000000-0000-0000-0000-000000000000","definition":definition}),
        ),
        (
            "http_function_delete",
            json!({"name":"protected","revision":"00000000-0000-0000-0000-000000000000"}),
        ),
        (
            "http_function_invoke",
            json!({"name":"protected","method":"POST"}),
        ),
    ] {
        let mut body = sessionless_run_js_body();
        body["params"]["name"] = json!(name);
        body["params"]["arguments"] = arguments;
        for token in [None, Some("unverifiable-token")] {
            let response =
                management_request(&client, &server.mcp_url(), token, name, body.clone())
                    .send()
                    .await
                    .unwrap();
            assert_eq!(response.status().as_u16(), 401, "{name}");
            assert!(!response.text().await.unwrap().contains("function handler"));
        }
    }
    // Denied create did not mutate the shared registry.
    let token = valid_token();
    let url = format!(
        "{}/api/http-functions/v1/definitions/protected",
        server.base_url
    );
    assert_eq!(
        client
            .get(&url)
            .bearer_auth(&token)
            .send()
            .await
            .unwrap()
            .status()
            .as_u16(),
        404
    );
    let mut body = sessionless_run_js_body();
    body["params"]["name"] = json!("http_function_create");
    body["params"]["arguments"] = json!({"name":"protected","definition":definition});
    let response = management_request(
        &client,
        &server.mcp_url(),
        Some(&token),
        "http_function_create",
        body,
    )
    .send()
    .await
    .unwrap();
    assert!(response.status().is_success());
    let text = response.text().await.unwrap();
    assert!(!text.contains("\"isError\":true"), "{text}");
    let metadata = client.get(&url).bearer_auth(&token).send().await.unwrap();
    assert_eq!(metadata.status().as_u16(), 200);
    assert!(
        metadata
            .json::<Value>()
            .await
            .unwrap()
            .get("source")
            .is_none()
    );
    server.stop().await;
    jwks_task.abort();
}
