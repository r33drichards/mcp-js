//! Real binary contract between the HTTP registry and its MCP management tools.
use base64::{Engine as _, engine::general_purpose::STANDARD};
use reqwest::Client;
use serde_json::{Value, json};
use std::process::Stdio;
use tokio::{
    process::Command,
    time::{Duration, sleep},
};

struct Server {
    child: tokio::process::Child,
    url: String,
    client: Client,
    _dir: tempfile::TempDir,
}
impl Server {
    async fn start(enabled: bool) -> Self {
        Self::start_mode(enabled, false).await
    }
    async fn start_mode(enabled: bool, stateful: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let mut command = Command::new(env!("CARGO_BIN_EXE_server"));
        command
            .args([
                "--http-port",
                &port.to_string(),
                "--bind-host",
                "127.0.0.1",
                "--heap-store",
                if stateful { "dir" } else { "none" },
                "--session-db-path",
            ])
            .arg(dir.path().join("sessions"))
            .env_remove("MCP_V8_HTTP_FUNCTIONS_DIR")
            .env_remove("MCP_SCHEDULED_WORKER_CONFIG")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);
        if stateful {
            command.arg("--heap-dir").arg(dir.path().join("heaps"));
        }
        if enabled {
            command.env("MCP_V8_HTTP_FUNCTIONS_DIR", dir.path().join("functions"));
        }
        let child = command.spawn().unwrap();
        let server = Self {
            child,
            url: format!("http://127.0.0.1:{port}"),
            client: Client::builder()
                .timeout(Duration::from_secs(20))
                .build()
                .unwrap(),
            _dir: dir,
        };
        for _ in 0..150 {
            if server
                .client
                .get(format!("{}/api/executions", server.url))
                .send()
                .await
                .is_ok()
            {
                return server;
            }
            sleep(Duration::from_millis(100)).await;
        }
        panic!("server did not start");
    }
    async fn rpc(&self, method: &str, name: &str, args: Value) -> Value {
        let mut params = json!({"_meta": {
            "io.modelcontextprotocol/protocolVersion":"2026-07-28",
            "io.modelcontextprotocol/clientInfo":{"name":"function-management-test","version":"1"},
            "io.modelcontextprotocol/clientCapabilities":{}
        }});
        if method == "tools/call" {
            params["name"] = json!(name);
            params["arguments"] = args;
        }
        let response = self
            .client
            .post(format!("{}/mcp", self.url))
            .header("Accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", "2026-07-28")
            .header("Mcp-Method", method)
            .header("Mcp-Name", name)
            .json(&json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}))
            .send()
            .await
            .unwrap();
        assert!(response.status().is_success(), "{}", response.status());
        let text = response.text().await.unwrap();
        let raw = if text.contains("data:") {
            text.lines()
                .filter_map(|line| line.strip_prefix("data:"))
                .collect::<String>()
        } else {
            text
        };
        let reply: Value = serde_json::from_str(&raw).unwrap();
        assert!(reply.get("error").is_none(), "{reply}");
        reply["result"].clone()
    }
    async fn call(&self, name: &str, args: Value, error: bool) -> Value {
        let result = self.rpc("tools/call", name, args).await;
        assert_eq!(
            result["isError"].as_bool().unwrap_or(false),
            error,
            "{result}"
        );
        serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap()
    }
    async fn stop(mut self) {
        self.child.kill().await.unwrap();
        self.child.wait().await.unwrap();
    }
}
fn definition() -> Value {
    json!({"version":1,"methods":["POST","HEAD"],"export":"handler",
        "source":"function handler(r) { return {status:201, headers:{'content-type':'application/json'}, body:JSON.stringify(r)}; }",
        "limits":{"timeout_seconds":3,"heap_mb":32}})
}

#[tokio::test]
async fn escaped_source_limits_match_for_http_and_mcp_registration() {
    const SOURCE_LIMIT: usize = 64 * 1024;
    const ENVELOPE_LIMIT: usize = 6 * SOURCE_LIMIT + 4096;
    let server = Server::start(true).await;
    let handler = "function handler(){return {status:200,body:'escaped'}}";
    let padding = SOURCE_LIMIT - handler.len() - 4;

    for (label, character) in [("backslash", '\\'), ("control", '\u{0001}')] {
        let mut def = definition();
        let source = format!("/*{}*/{handler}", character.to_string().repeat(padding));
        assert_eq!(source.len(), SOURCE_LIMIT);
        def["source"] = json!(source);
        assert!(def.to_string().len() > SOURCE_LIMIT + 4096);
        assert!(def.to_string().len() < ENVELOPE_LIMIT);

        let http_name = format!("http-{label}");
        let http_url = format!(
            "{}/api/http-functions/v1/definitions/{http_name}",
            server.url
        );
        let created = server
            .client
            .put(&http_url)
            .header("If-None-Match", "*")
            .json(&def)
            .send()
            .await
            .unwrap();
        assert_eq!(created.status().as_u16(), 200, "HTTP create: {label}");
        let created: Value = created.json().await.unwrap();
        let updated = server
            .call(
                "http_function_update",
                json!({
                    "name":http_name,"revision":created["revision"],"definition":def
                }),
                false,
            )
            .await;
        assert_ne!(created["revision"], updated["revision"]);

        let mcp_name = format!("mcp-{label}");
        let created = server
            .call(
                "http_function_create",
                json!({
                    "name":mcp_name,"definition":def
                }),
                false,
            )
            .await;
        let updated = server
            .client
            .put(format!(
                "{}/api/http-functions/v1/definitions/{mcp_name}",
                server.url
            ))
            .header(
                "If-Match",
                format!("\"{}\"", created["revision"].as_str().unwrap()),
            )
            .json(&def)
            .send()
            .await
            .unwrap();
        assert_eq!(updated.status().as_u16(), 200, "HTTP update: {label}");
        let updated: Value = updated.json().await.unwrap();
        assert_ne!(created["revision"], updated["revision"]);
        let invoked = server
            .call(
                "http_function_invoke",
                json!({
                    "name":mcp_name,"method":"POST"
                }),
                false,
            )
            .await;
        assert_eq!(invoked["status"], 200);
        assert_eq!(invoked["body_base64"], STANDARD.encode("escaped"));
    }

    // An envelope that fits must still obey the decoded 64 KiB source quota.
    let mut oversized_source = definition();
    oversized_source["source"] = json!(format!("/*{}*/{handler}", "\u{0001}".repeat(padding + 1)));
    assert_eq!(
        oversized_source["source"].as_str().unwrap().len(),
        SOURCE_LIMIT + 1
    );
    assert!(oversized_source.to_string().len() < ENVELOPE_LIMIT);
    let rejected = server
        .client
        .put(format!(
            "{}/api/http-functions/v1/definitions/source-too-big",
            server.url
        ))
        .header("If-None-Match", "*")
        .json(&oversized_source)
        .send()
        .await
        .unwrap();
    assert_eq!(rejected.status().as_u16(), 400);
    let rejected = server
        .call(
            "http_function_create",
            json!({
                "name":"source-too-big","definition":oversized_source
            }),
            true,
        )
        .await;
    assert_eq!(rejected["status"], 400);

    let envelope_url = format!("{}/api/http-functions/v1/definitions/envelope", server.url);
    let json = definition().to_string();
    let exact_envelope = format!("{}{json}", " ".repeat(ENVELOPE_LIMIT - json.len()));
    assert_eq!(exact_envelope.len(), ENVELOPE_LIMIT);
    let accepted = server
        .client
        .put(&envelope_url)
        .header("If-None-Match", "*")
        .body(exact_envelope.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(accepted.status().as_u16(), 200);
    let rejected = server
        .client
        .put(&envelope_url)
        .header("If-None-Match", "*")
        .body(format!(" {exact_envelope}"))
        .send()
        .await
        .unwrap();
    assert_eq!(rejected.status().as_u16(), 413);
    let rejected = server
        .client
        .put(&envelope_url)
        .header("If-None-Match", "*")
        .body("{malformed")
        .send()
        .await
        .unwrap();
    assert_eq!(rejected.status().as_u16(), 400);

    // MCP canonicalizes JSON before calling the same PUT operation. Oversized
    // metadata must still hit its envelope bound, not reach source validation.
    let mut oversized_envelope = definition();
    oversized_envelope["export"] = json!("x".repeat(ENVELOPE_LIMIT));
    let rejected = server
        .call(
            "http_function_create",
            json!({
                "name":"envelope-too-big","definition":oversized_envelope
            }),
            true,
        )
        .await;
    assert_eq!(rejected["status"], 413);
    let rejected = server
        .call(
            "http_function_create",
            json!({
                "name":"malformed","definition":{"version":1}
            }),
            true,
        )
        .await;
    assert_eq!(rejected["status"], 400);
    server.stop().await;
}

#[tokio::test]
async fn http_and_mcp_share_revisions_validation_and_invocation() {
    let server = Server::start(true).await;
    let tools = server.rpc("tools/list", "", json!({})).await;
    for name in ["create", "list", "lookup", "update", "delete", "invoke"] {
        assert!(
            tools["tools"]
                .as_array()
                .unwrap()
                .iter()
                .any(|tool| tool["name"] == format!("http_function_{name}")),
            "{tools}"
        );
    }
    let created = server
        .call(
            "http_function_create",
            json!({"name":"echo","definition":definition()}),
            false,
        )
        .await;
    assert!(created.get("source").is_none());
    let revision = created["revision"].as_str().unwrap();
    let url = format!("{}/api/http-functions/v1/definitions/echo", server.url);
    let response = server.client.get(&url).send().await.unwrap();
    assert_eq!(response.headers()["etag"], format!("\"{revision}\""));
    assert_eq!(response.json::<Value>().await.unwrap(), created);
    let duplicate = server
        .call(
            "http_function_create",
            json!({"name":"echo","definition":definition()}),
            true,
        )
        .await;
    assert_eq!(duplicate["status"], 412);
    let list = server.call("http_function_list", json!({}), false).await;
    assert_eq!(list.as_array().unwrap(), &[created.clone()]);

    // Concurrent protocols compete on one exact revision; exactly one commits.
    let http = server
        .client
        .put(&url)
        .header("If-Match", format!("\"{revision}\""))
        .json(&definition())
        .send();
    let mcp = server.rpc(
        "tools/call",
        "http_function_update",
        json!({"name":"echo","revision":revision,"definition":definition()}),
    );
    let (http, mcp) = tokio::join!(http, mcp);
    let http = http.unwrap();
    let mcp_failed = mcp["isError"].as_bool().unwrap_or(false);
    assert_eq!(
        http.status().is_success(),
        mcp_failed,
        "HTTP {} MCP {mcp}",
        http.status()
    );
    if !http.status().is_success() {
        assert_eq!(http.status().as_u16(), 412);
    }
    if mcp_failed {
        let failure: Value =
            serde_json::from_str(mcp["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(failure["status"], 412);
    }
    let stale = server
        .call(
            "http_function_delete",
            json!({"name":"echo","revision":revision}),
            true,
        )
        .await;
    assert_eq!(stale["status"], 412);
    let stale = server
        .call(
            "http_function_update",
            json!({"name":"echo","revision":revision,"definition":definition()}),
            true,
        )
        .await;
    assert_eq!(stale["status"], 412);

    let invoked = server.call("http_function_invoke", json!({"name":"echo","method":"POST","query":[["a","1"],["a","2"]],"headers":{"content-type":"application/octet-stream"},"body_base64":"AP8="}), false).await;
    assert_eq!(invoked["status"], 201);
    let body: Value = serde_json::from_slice(
        &STANDARD
            .decode(invoked["body_base64"].as_str().unwrap())
            .unwrap(),
    )
    .unwrap();
    assert_eq!(body["path"], "/functions/v1/echo");
    assert_eq!(body["query"], json!([["a", "1"], ["a", "2"]]));
    assert_eq!(body["body_base64"], "AP8=");
    assert_eq!(body["headers"]["content-type"], "application/octet-stream");
    let invoked = server
        .call(
            "http_function_invoke",
            json!({"name":"echo","method":"HEAD"}),
            false,
        )
        .await;
    assert_eq!(invoked["body_base64"], "");
    let method = server
        .call(
            "http_function_invoke",
            json!({"name":"echo","method":"GET"}),
            false,
        )
        .await;
    assert_eq!(method["status"], 405);

    for (name, args) in [
        ("http_function_list", json!({"unexpected":true})),
        ("http_function_lookup", json!({"name":"../echo"})),
        (
            "http_function_create",
            json!({"name":"bad","definition":{"version":1}}),
        ),
        (
            "http_function_update",
            json!({"name":"echo","definition":definition()}),
        ),
        (
            "http_function_delete",
            json!({"name":"echo","revision":"*"}),
        ),
        (
            "http_function_invoke",
            json!({"name":"echo","method":"POST","body_base64":"!"}),
        ),
        (
            "http_function_invoke",
            json!({"name":"echo","method":"POST","headers":{"authorization":"secret"}}),
        ),
    ] {
        let failure = server.call(name, args, true).await;
        assert_eq!(failure["status"], 400, "{failure}");
        assert!(!failure.to_string().contains("secret"));
    }
    let current = server
        .call("http_function_lookup", json!({"name":"echo"}), false)
        .await;
    let deleted = server
        .call(
            "http_function_delete",
            json!({"name":"echo","revision":current["revision"]}),
            false,
        )
        .await;
    assert_eq!(deleted["deleted"], true);
    assert_eq!(
        server
            .client
            .get(&url)
            .send()
            .await
            .unwrap()
            .status()
            .as_u16(),
        404
    );
    let missing = server
        .call("http_function_lookup", json!({"name":"echo"}), true)
        .await;
    assert_eq!(missing["status"], 404);
    // Delete/recreate must not resurrect any old precondition.
    let recreated = server
        .call(
            "http_function_create",
            json!({"name":"echo","definition":definition()}),
            false,
        )
        .await;
    assert_ne!(recreated["revision"], created["revision"]);
    let stale = server
        .call(
            "http_function_delete",
            json!({"name":"echo","revision":current["revision"]}),
            true,
        )
        .await;
    assert_eq!(stale["status"], 412);
    server.stop().await;
}

#[tokio::test]
async fn disabled_registry_advertises_no_management_tools_and_fails_closed() {
    let server = Server::start(false).await;
    let tools = server.rpc("tools/list", "", json!({})).await;
    assert!(
        !tools["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["name"].as_str().unwrap().starts_with("http_function_"))
    );
    let unavailable = server.call("http_function_list", json!({}), true).await;
    assert_eq!(unavailable["status"], 503);
    server.stop().await;
}

#[tokio::test]
async fn session_capable_mcp_exposes_the_same_registry() {
    let server = Server::start_mode(true, true).await;
    let created = server
        .call(
            "http_function_create",
            json!({"name":"stateful","definition":definition()}),
            false,
        )
        .await;
    let http = server
        .client
        .get(format!(
            "{}/api/http-functions/v1/definitions/stateful",
            server.url
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(http.json::<Value>().await.unwrap(), created);
    let invoked = server
        .call(
            "http_function_invoke",
            json!({"name":"stateful","method":"POST"}),
            false,
        )
        .await;
    assert_eq!(invoked["status"], 201);
    server.stop().await;
}
