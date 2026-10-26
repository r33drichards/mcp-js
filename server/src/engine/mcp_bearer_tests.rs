//! Private bearer adapter regression tests. All credentials are fixtures.

use super::*;

use std::collections::VecDeque;
use std::io::Write;
use std::sync::Mutex;

use axum::extract::{Request, State};
use axum::response::Response;
use axum::{Router, routing::any};
use http::{Method, StatusCode};
use tempfile::NamedTempFile;

const TOKEN: &str = "fixture-token-SECRET-9472";

fn secret_file(bytes: &[u8]) -> NamedTempFile {
    let mut file = NamedTempFile::new().unwrap();
    file.write_all(bytes).unwrap();
    file.flush().unwrap();
    file
}

fn file_name(file: &NamedTempFile) -> &str {
    file.path().to_str().unwrap()
}

fn ping() -> ClientJsonRpcMessage {
    serde_json::from_value(serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "ping"
    }))
    .unwrap()
}

#[derive(Clone)]
struct Reply {
    status: StatusCode,
    headers: Vec<(String, String)>,
    body: String,
}

impl Reply {
    fn new(status: StatusCode, content_type: &str, body: impl Into<String>) -> Self {
        Self {
            status,
            headers: vec![("content-type".into(), content_type.into())],
            body: body.into(),
        }
    }

    fn json(body: impl Into<String>) -> Self {
        Self::new(StatusCode::OK, "application/json", body)
    }

    fn sse(body: impl Into<String>) -> Self {
        Self::new(StatusCode::OK, "text/event-stream", body)
    }
}

#[derive(Clone, Debug)]
struct Seen {
    method: Method,
    path: String,
    authorization: Option<String>,
}

#[derive(Clone, Default)]
struct MockState {
    replies: Arc<Mutex<VecDeque<Reply>>>,
    seen: Arc<Mutex<Vec<Seen>>>,
}

struct MockServer {
    url: String,
    state: MockState,
    task: tokio::task::JoinHandle<()>,
}

impl MockServer {
    async fn start(replies: Vec<Reply>) -> Self {
        async fn handle(State(state): State<MockState>, request: Request) -> Response {
            state.seen.lock().unwrap().push(Seen {
                method: request.method().clone(),
                path: request.uri().path().into(),
                authorization: request
                    .headers()
                    .get(AUTHORIZATION)
                    .map(|value| value.to_str().unwrap().to_owned()),
            });
            let reply = state
                .replies
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Reply::new(StatusCode::NO_CONTENT, "application/json", ""));
            let mut response = Response::builder().status(reply.status);
            for (name, value) in reply.headers {
                response = response.header(name, value);
            }
            response.body(axum::body::Body::from(reply.body)).unwrap()
        }

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        let state = MockState {
            replies: Arc::new(Mutex::new(replies.into())),
            ..Default::default()
        };
        let router = Router::new()
            .fallback(any(handle))
            .with_state(state.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self { url, state, task }
    }

    fn seen(&self) -> Vec<Seen> {
        self.state.seen.lock().unwrap().clone()
    }

    fn client(&self, file: &NamedTempFile) -> BearerHttpClient {
        BearerHttpClient::from_file("fixture", &self.url, file_name(file)).unwrap()
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn post(
    client: &BearerHttpClient,
    url: &str,
) -> Result<StreamableHttpPostResponse, TransportError> {
    tokio::time::timeout(
        Duration::from_secs(3),
        client.post_message(url.into(), ping(), None, None, HashMap::new()),
    )
    .await
    .expect("fixture HTTP request timed out")
}

#[test]
fn token_loader_accepts_only_one_optional_line_ending_and_redacts_debug() {
    for suffix in ["", "\n", "\r\n"] {
        let file = secret_file(format!("{TOKEN}{suffix}").as_bytes());
        let header = read_authorization("fixture", file_name(&file)).unwrap();
        assert_eq!(header.to_str().unwrap(), format!("Bearer {TOKEN}"));
        assert!(header.is_sensitive());
        assert!(!format!("{header:?}").contains(TOKEN));
        let client =
            BearerHttpClient::from_file("fixture", "http://127.0.0.1:1/mcp", file_name(&file))
                .unwrap();
        assert!(!format!("{client:?} {:?}", client.secret).contains(TOKEN));
    }
    let file = secret_file(b"aZ09-._~+/===");
    assert!(read_authorization("fixture", file_name(&file)).is_ok());
}

#[test]
fn token_loader_fails_closed_for_missing_empty_invalid_and_oversize_files() {
    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("missing");
    assert!(read_authorization("fixture", missing.to_str().unwrap()).is_err());
    assert!(read_authorization("fixture", directory.path().to_str().unwrap()).is_err());
    for bytes in [
        &b""[..],
        &b"\n"[..],
        &b"\r\n"[..],
        &b"="[..],
        &b"==="[..],
        &b" abc"[..],
        &b"abc "[..],
        &b"a=b"[..],
        &b"abc\r"[..],
        &b"abc\n\n"[..],
        &b"abc\r\nInjected: yes"[..],
        &b"abc\tdef"[..],
        &b"abc\0def"[..],
        &b"\xff"[..],
    ] {
        let file = secret_file(bytes);
        let error = read_authorization("fixture", file_name(&file)).unwrap_err();
        assert!(error.contains("bearer auth"));
        assert!(!error.contains("Injected"));
    }
    let file = secret_file(&vec![b'a'; MAX_TOKEN_BYTES as usize + 1]);
    assert!(
        read_authorization("fixture", file_name(&file))
            .unwrap_err()
            .contains("64 KiB")
    );
    let file = secret_file(&vec![b'a'; MAX_TOKEN_BYTES as usize]);
    assert!(read_authorization("fixture", file_name(&file)).is_ok());
}

#[cfg(unix)]
#[test]
fn token_loader_follows_projected_secret_symlinks_and_rereads_rotation() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    for (version, token) in [("version-1", TOKEN), ("version-2", "rotated-fixture-token")] {
        std::fs::create_dir(root.path().join(version)).unwrap();
        std::fs::write(root.path().join(version).join("token"), token).unwrap();
    }
    symlink("version-1", root.path().join("..data")).unwrap();
    symlink("..data/token", root.path().join("token")).unwrap();
    let path = root.path().join("token");
    let first = read_authorization("fixture", path.to_str().unwrap()).unwrap();
    assert_eq!(first.to_str().unwrap(), format!("Bearer {TOKEN}"));
    symlink("version-2", root.path().join("..data_tmp")).unwrap();
    std::fs::rename(root.path().join("..data_tmp"), root.path().join("..data")).unwrap();
    let next = read_authorization("fixture", path.to_str().unwrap()).unwrap();
    assert_eq!(next.to_str().unwrap(), "Bearer rotated-fixture-token");
}

#[cfg(unix)]
#[test]
fn token_loader_rejects_fifo_without_waiting_for_a_writer() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("fifo");
    let c_path = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
    // SAFETY: c_path is a valid, live NUL-terminated pathname in this tempdir.
    assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
    let started = std::time::Instant::now();
    assert!(
        read_authorization("fixture", path.to_str().unwrap())
            .unwrap_err()
            .contains("regular file")
    );
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[cfg(unix)]
#[test]
fn token_loader_rejects_unreadable_file_when_not_running_as_root() {
    use std::os::unix::fs::PermissionsExt;
    // Root legitimately bypasses file mode bits; missing-file coverage is unconditional.
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let file = secret_file(TOKEN.as_bytes());
    std::fs::set_permissions(file.path(), std::fs::Permissions::from_mode(0o000)).unwrap();
    assert!(read_authorization("fixture", file_name(&file)).is_err());
}

#[test]
fn reference_validation_rejects_relative_paths_non_http_userinfo_and_fragments() {
    assert!(validate_reference("fixture", "https://example.com/mcp", "relative-token").is_err());
    for url in [
        "file:///tmp/mcp",
        "ftp://example.com/mcp",
        "not a url",
        "https://user@example.com/mcp",
        "https://user:pass@example.com/mcp",
        "https://example.com/mcp#fragment",
    ] {
        assert!(validate_reference("fixture", url, "/secret/token").is_err());
    }
    for url in ["http://127.0.0.1:8080/mcp", "https://example.com/mcp"] {
        assert!(validate_reference("fixture", url, "/secret/token").is_ok());
    }
}

#[tokio::test]
async fn invalid_token_files_fail_before_any_network_request() {
    let server = MockServer::start(vec![]).await;
    let file = secret_file(b"\n");
    assert!(BearerHttpClient::from_file("fixture", &server.url, file_name(&file)).is_err());
    assert!(server.seen().is_empty());
}

#[tokio::test]
async fn bearer_is_scoped_to_exact_endpoint_for_get_post_and_delete() {
    let server = MockServer::start(vec![
        Reply::json(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#),
        Reply::sse(""),
        Reply::new(StatusCode::NO_CONTENT, "application/json", ""),
    ])
    .await;
    let trap = MockServer::start(vec![]).await;
    let file = secret_file(TOKEN.as_bytes());
    let client = server.client(&file);
    post(&client, &server.url).await.unwrap();
    let _stream = client
        .get_stream(server.url.clone().into(), None, None, None, HashMap::new())
        .await
        .unwrap();
    client
        .delete_session(
            server.url.clone().into(),
            "fixture-session".into(),
            None,
            HashMap::new(),
        )
        .await
        .unwrap();
    assert!(post(&client, &trap.url).await.is_err());
    assert!(
        client
            .get_stream(trap.url.clone().into(), None, None, None, HashMap::new())
            .await
            .is_err()
    );
    assert!(
        client
            .delete_session(
                trap.url.clone().into(),
                "fixture-session".into(),
                None,
                HashMap::new()
            )
            .await
            .is_err()
    );
    assert!(
        post(&client, &format!("{}/other", server.url))
            .await
            .is_err()
    );
    let seen = server.seen();
    assert_eq!(seen.len(), 3);
    assert_eq!(
        seen.iter()
            .map(|request| request.method.clone())
            .collect::<Vec<_>>(),
        vec![Method::POST, Method::GET, Method::DELETE]
    );
    assert!(seen.iter().all(|request| request.path == "/mcp"
        && request.authorization.as_deref() == Some(&format!("Bearer {TOKEN}"))));
    assert!(trap.seen().is_empty());
    reqwest_mcp::Client::new()
        .get(&trap.url)
        .send()
        .await
        .unwrap();
    assert_eq!(trap.seen().len(), 1);
    assert!(trap.seen()[0].authorization.is_none());
}

#[tokio::test]
async fn redirects_never_forward_the_token_for_any_http_method() {
    let trap = MockServer::start(vec![]).await;
    let file = secret_file(TOKEN.as_bytes());
    for status in [301, 302, 303, 307, 308] {
        let redirect = || Reply {
            status: StatusCode::from_u16(status).unwrap(),
            headers: vec![("location".into(), trap.url.clone())],
            body: String::new(),
        };
        let server = MockServer::start(vec![redirect(), redirect(), redirect()]).await;
        let client = server.client(&file);
        let _ = post(&client, &server.url).await;
        let _ = client
            .get_stream(server.url.clone().into(), None, None, None, HashMap::new())
            .await;
        let _ = client
            .delete_session(
                server.url.clone().into(),
                "fixture-session".into(),
                None,
                HashMap::new(),
            )
            .await;
        assert_eq!(server.seen().len(), 3);
        assert!(
            trap.seen().is_empty(),
            "redirect status {status} reached trap"
        );
    }
    let same_origin = Reply {
        status: StatusCode::TEMPORARY_REDIRECT,
        headers: vec![("location".into(), "/different-endpoint".into())],
        body: String::new(),
    };
    let server =
        MockServer::start(vec![same_origin.clone(), same_origin.clone(), same_origin]).await;
    let client = server.client(&file);
    let _ = post(&client, &server.url).await;
    let _ = client
        .get_stream(server.url.clone().into(), None, None, None, HashMap::new())
        .await;
    let _ = client
        .delete_session(
            server.url.clone().into(),
            "fixture-session".into(),
            None,
            HashMap::new(),
        )
        .await;
    assert_eq!(server.seen().len(), 3);
    assert!(server.seen().iter().all(|request| request.path == "/mcp"));
}

#[tokio::test]
async fn rejects_auth_overrides_and_token_in_endpoint_before_network_io() {
    let server = MockServer::start(vec![]).await;
    let file = secret_file(TOKEN.as_bytes());
    let client = server.client(&file);
    assert!(
        client
            .post_message(
                server.url.clone().into(),
                ping(),
                None,
                Some("override".into()),
                HashMap::new()
            )
            .await
            .is_err()
    );
    assert!(
        client
            .get_stream(
                server.url.clone().into(),
                None,
                None,
                Some("override".into()),
                HashMap::new()
            )
            .await
            .is_err()
    );
    assert!(
        client
            .delete_session(
                server.url.clone().into(),
                "session".into(),
                Some("override".into()),
                HashMap::new()
            )
            .await
            .is_err()
    );
    for name in [AUTHORIZATION, http::header::HOST] {
        let headers = HashMap::from([(name, HeaderValue::from_static("override"))]);
        assert!(
            client
                .post_message(
                    server.url.clone().into(),
                    ping(),
                    None,
                    None,
                    headers.clone()
                )
                .await
                .is_err()
        );
        assert!(
            client
                .get_stream(server.url.clone().into(), None, None, None, headers.clone())
                .await
                .is_err()
        );
        assert!(
            client
                .delete_session(server.url.clone().into(), "session".into(), None, headers)
                .await
                .is_err()
        );
    }
    assert!(
        BearerHttpClient::from_file(
            "fixture",
            &format!("{}?secret={TOKEN}", server.url),
            file_name(&file)
        )
        .is_err()
    );
    assert!(server.seen().is_empty());
}

#[tokio::test]
async fn json_rpc_errors_redact_plain_escaped_nested_values_and_keys() {
    let body = serde_json::json!({
        "jsonrpc": "2.0", "id": 1,
        "error": {"code": -32000, "message": format!("echo {TOKEN}"),
            "data": {format!("key-{TOKEN}"): [TOKEN, {"nested": format!("Bearer {TOKEN}")}]}}
    })
    .to_string();
    let escaped = body.replace("fixture", "\\u0066ixture");
    for status in [StatusCode::OK, StatusCode::BAD_REQUEST] {
        let server = MockServer::start(vec![Reply::new(
            status,
            "application/json",
            escaped.clone(),
        )])
        .await;
        let file = secret_file(TOKEN.as_bytes());
        let response = post(&server.client(&file), &server.url).await.unwrap();
        let debug = format!("{response:?}");
        assert!(!debug.contains(TOKEN));
        let StreamableHttpPostResponse::Json(message, _) = response else {
            panic!("expected JSON");
        };
        let value = serde_json::to_value(message).unwrap();
        assert!(!value.to_string().contains(TOKEN));
        assert!(
            value["error"]["message"]
                .as_str()
                .unwrap()
                .contains("[REDACTED]")
        );
        assert!(value["error"]["data"].get("key-[REDACTED]").is_some());
    }
}

#[tokio::test]
async fn response_body_challenges_and_session_ids_cannot_leak_the_token() {
    let mut challenge = Reply::new(StatusCode::UNAUTHORIZED, "text/plain", TOKEN);
    challenge.headers.push((
        "www-authenticate".into(),
        format!("Bearer error=\"{TOKEN}\""),
    ));
    let mut session = Reply::json(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#);
    session
        .headers
        .push(("mcp-session-id".into(), TOKEN.into()));
    let replies = vec![
        Reply::new(StatusCode::INTERNAL_SERVER_ERROR, "text/plain", TOKEN),
        Reply::json(format!("{{invalid-json:{TOKEN}")),
        challenge,
        session,
    ];
    let server = MockServer::start(replies).await;
    let file = secret_file(TOKEN.as_bytes());
    let client = server.client(&file);
    for _ in 0..4 {
        let error = post(&client, &server.url)
            .await
            .err()
            .expect("must reject remote credential echo");
        assert!(!format!("{error} {error:?}").contains(TOKEN));
    }
}

#[tokio::test]
async fn sse_get_and_post_redact_decoded_json_before_rmcp_receives_it() {
    let data = serde_json::json!({"jsonrpc":"2.0", "id":1,
        "error":{"code":-32000,"message":TOKEN,"data":{TOKEN: [TOKEN]}}})
    .to_string()
    .replace("fixture", "\\u0066ixture");
    let body = format!("id: safe-event\r\nevent: message\r\ndata: {data}\r\n\r\n");
    let server = MockServer::start(vec![Reply::sse(body.clone()), Reply::sse(body)]).await;
    let file = secret_file(TOKEN.as_bytes());
    let client = server.client(&file);
    let mut get = client
        .get_stream(server.url.clone().into(), None, None, None, HashMap::new())
        .await
        .unwrap();
    let event = get.next().await.unwrap().unwrap();
    assert!(!format!("{event:?}").contains(TOKEN));
    assert!(event.data.unwrap().contains("[REDACTED]"));
    let StreamableHttpPostResponse::Sse(mut stream, _) = post(&client, &server.url).await.unwrap()
    else {
        panic!("expected SSE");
    };
    let event = stream.next().await.unwrap().unwrap();
    assert!(!format!("{event:?}").contains(TOKEN));
    assert!(event.data.unwrap().contains("[REDACTED]"));
}

#[tokio::test]
async fn sse_rejects_secret_ids_event_types_malformed_data_and_oversized_events() {
    let bodies = vec![
        format!("id: {TOKEN}\ndata: {{}}\n\n"),
        format!("event: {TOKEN}\ndata: {{}}\n\n"),
        format!("data: malformed-{TOKEN}\n\n"),
        format!("{TOKEN}\n\n"),
    ];
    let server = MockServer::start(bodies.into_iter().map(Reply::sse).collect()).await;
    let file = secret_file(TOKEN.as_bytes());
    let client = server.client(&file);
    for _ in 0..4 {
        let mut stream = client
            .get_stream(server.url.clone().into(), None, None, None, HashMap::new())
            .await
            .unwrap();
        let error = stream.next().await.unwrap().unwrap_err();
        assert!(!format!("{error} {error:?}").contains(TOKEN));
    }
    let oversized = format!("data: {}\n\n", "x".repeat(128));
    let server =
        MockServer::start(vec![Reply::sse(oversized.clone()), Reply::sse(oversized)]).await;
    let client = server.client(&file);
    let mut stream = client
        .get_stream_with_max_sse_event_size(
            server.url.clone().into(),
            None,
            None,
            None,
            HashMap::new(),
            32,
        )
        .await
        .unwrap();
    assert!(stream.next().await.unwrap().is_err());
    let StreamableHttpPostResponse::Sse(mut stream, _) = client
        .post_message_with_max_sse_event_size(
            server.url.clone().into(),
            ping(),
            None,
            None,
            HashMap::new(),
            32,
        )
        .await
        .unwrap()
    else {
        panic!("expected SSE");
    };
    assert!(stream.next().await.unwrap().is_err());
}

#[derive(Clone, Default)]
struct LogCapture(Arc<Mutex<Vec<u8>>>);

impl Write for LogCapture {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn dependency_content_type_and_sse_parser_logs_never_include_the_token() {
    let server = MockServer::start(vec![
        Reply::new(StatusCode::OK, &format!("text/{TOKEN}"), "not-json"),
        Reply::sse(format!(": {TOKEN}\n\n{TOKEN}\n\n")),
    ])
    .await;
    let file = secret_file(TOKEN.as_bytes());
    let client = server.client(&file);
    let capture = LogCapture::default();
    let writer = capture.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_max_level(tracing::Level::TRACE)
        .with_writer(move || writer.clone())
        .finish();
    async {
        tracing::info!("bearer-log-capture-active");
        let error = post(&client, &server.url).await.err().unwrap();
        tracing::warn!(?error, "sanitized HTTP error");
        let mut stream = client
            .get_stream(server.url.clone().into(), None, None, None, HashMap::new())
            .await
            .unwrap();
        let error = stream.next().await.unwrap().unwrap_err();
        tracing::warn!(?error, "sanitized SSE error");
    }
    .with_subscriber(subscriber)
    .await;
    let logs = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
    assert!(
        logs.contains("bearer-log-capture-active"),
        "capture must be active"
    );
    assert!(
        !logs.contains(TOKEN),
        "credential appeared in tracing output: {logs}"
    );
}
