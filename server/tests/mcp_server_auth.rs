//! Configuration compatibility tests for downstream HTTP MCP servers.

use server::engine::mcp_client::{McpServerAuth, McpServerConfig, McpServerTransport};

#[test]
fn deserializes_minimal_http_oauth_browser_config() {
    let config: McpServerConfig = serde_json::from_str(
        r#"{
            "name": "calendar",
            "transport": "http",
            "url": "https://calendar.example.com/mcp",
            "auth": { "type": "oauth_browser" }
        }"#,
    )
    .expect("minimal HTTP OAuth browser configuration should deserialize");

    assert!(matches!(
        config.transport,
        McpServerTransport::Http { ref url }
            if url == "https://calendar.example.com/mcp"
    ));
    assert!(matches!(
        config.auth,
        Some(McpServerAuth::OauthBrowser {
            scope: None,
            client_id: None,
            client_secret: None,
            redirect_port: None,
            token_cache: None,
        })
    ));
}

#[test]
fn deserializes_fully_specified_http_oauth_browser_config() {
    let config: McpServerConfig = serde_json::from_str(
        r#"{
            "name": "calendar",
            "transport": "http",
            "url": "https://calendar.example.com/mcp",
            "auth": {
                "type": "oauth_browser",
                "scope": ["calendar.read", "calendar.write"],
                "client_id": "calendar-cli",
                "client_secret": "secret",
                "redirect_port": 8080,
                "token_cache": "/tmp/calendar-tokens.json"
            }
        }"#,
    )
    .expect("fully specified HTTP OAuth browser configuration should deserialize");

    assert!(matches!(
        config.auth,
        Some(McpServerAuth::OauthBrowser {
            scope: Some(ref scope),
            client_id: Some(ref client_id),
            client_secret: Some(ref client_secret),
            redirect_port: Some(8080),
            token_cache: Some(ref token_cache),
        }) if scope == &["calendar.read", "calendar.write"]
            && client_id == "calendar-cli"
            && client_secret == "secret"
            && token_cache == "/tmp/calendar-tokens.json"
    ));
}

#[test]
fn keeps_stdio_transport_when_auth_is_present() {
    let config: McpServerConfig = serde_json::from_str(
        r#"{
            "name": "legacy",
            "transport": "stdio",
            "command": "legacy-mcp",
            "auth": { "type": "oauth_browser" }
        }"#,
    )
    .expect("stdio auth is retained in configuration and ignored by the transport");

    assert!(matches!(
        config.transport,
        McpServerTransport::Stdio { ref command, .. } if command == "legacy-mcp"
    ));
    assert!(matches!(
        config.auth,
        Some(McpServerAuth::OauthBrowser { .. })
    ));
}

#[test]
fn keeps_legacy_sse_config_working() {
    let config: McpServerConfig = serde_json::from_str(
        r#"{
            "name": "legacy-sse",
            "transport": "sse",
            "url": "https://legacy.example.com/sse"
        }"#,
    )
    .expect("legacy SSE configuration should still deserialize");

    assert!(matches!(
        config.transport,
        McpServerTransport::Sse { ref url } if url == "https://legacy.example.com/sse"
    ));
    assert!(config.auth.is_none());
}

#[test]
fn permits_http_oauth_but_keeps_sse_auth_fail_closed() {
    let http: McpServerConfig = serde_json::from_str(
        r#"{
            "name": "protected-http",
            "transport": "http",
            "url": "https://protected.example.com/mcp",
            "auth": { "type": "oauth_browser" }
        }"#,
    )
    .expect("HTTP auth configuration should deserialize");
    http.validate_for_connection()
        .expect("HTTP OAuth runtime is available");

    let sse: McpServerConfig = serde_json::from_str(
        r#"{
            "name": "protected-sse",
            "transport": "sse",
            "url": "https://protected.example.com/mcp",
            "auth": { "type": "oauth_browser" }
        }"#,
    )
    .expect("SSE auth configuration should deserialize");
    let error = sse
        .validate_for_connection()
        .expect_err("SSE auth must remain fail closed");
    assert!(error.contains("OAuth runtime support is not implemented"));
    assert!(error.contains("SSE"));
}

#[test]
fn permits_stdio_auth_configuration_for_legacy_compatibility() {
    let config: McpServerConfig = serde_json::from_str(
        r#"{
            "name": "legacy",
            "transport": "stdio",
            "command": "legacy-mcp",
            "auth": { "type": "oauth_browser" }
        }"#,
    )
    .expect("stdio auth configuration should deserialize");

    config
        .validate_for_connection()
        .expect("stdio auth remains ignored with a warning for compatibility");
}

#[test]
fn debug_output_redacts_oauth_client_secrets() {
    let config: McpServerConfig = serde_json::from_str(
        r#"{
            "name": "calendar",
            "transport": "http",
            "url": "https://calendar.example.com/mcp",
            "auth": {
                "type": "oauth_browser",
                "client_id": "calendar-cli",
                "client_secret": "client-secret"
            }
        }"#,
    )
    .expect("OAuth configuration should deserialize");

    let debug = format!("{config:?}");
    assert!(!debug.contains("client-secret"));
    assert!(debug.contains("[REDACTED]"));
}

#[test]
fn rejects_client_secret_without_client_id() {
    let config: McpServerConfig = serde_json::from_str(
        r#"{
            "name": "calendar",
            "transport": "http",
            "url": "https://calendar.example.com/mcp",
            "auth": {
                "type": "oauth_browser",
                "client_secret": "orphan-secret"
            }
        }"#,
    )
    .expect("configuration should deserialize before connection validation");

    let error = config
        .validate_for_connection()
        .expect_err("client_secret without client_id must fail closed");
    assert!(error.contains("client_secret"));
    assert!(error.contains("client_id"));
}
