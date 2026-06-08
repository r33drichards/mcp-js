//! Streamable HTTP client transport for attaching to upstream MCP servers
//! that speak the MCP "Streamable HTTP" transport (MCP 2025-03-26+).
//!
//! rmcp 0.1.5 ships only a *server* implementation of Streamable HTTP plus an
//! SSE *client*; it has no built-in Streamable HTTP *client*. This module
//! provides one so mcp-v8 can connect to Streamable HTTP endpoints directly,
//! instead of forcing the user to stand up an external stdio/SSE bridge.
//!
//! Protocol summary (client side):
//!   * Every JSON-RPC message is POSTed to a single endpoint URL with
//!     `Accept: application/json, text/event-stream`.
//!   * A POST containing a request yields either a single JSON response
//!     (`application/json`) or an SSE stream (`text/event-stream`) carrying
//!     the response (and any related server notifications) before closing.
//!   * A POST containing only notifications/responses yields `202 Accepted`
//!     with an empty body.
//!   * The server assigns a session via the `Mcp-Session-Id` response header
//!     on the initialize response; the client echoes it on every later POST.
//!
//! The standalone server->client GET stream is intentionally not opened: this
//! client only consumes tools (initialize / list / call), none of which depend
//! on unsolicited server-initiated messages. The transport satisfies rmcp's
//! `IntoTransport` blanket impl by being both a `Sink<ClientJsonRpcMessage>`
//! and a `Stream<Item = ServerJsonRpcMessage>`.

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{ready, Context, Poll};

use futures::channel::{mpsc, oneshot};
use futures::{FutureExt, Sink, Stream, StreamExt};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, ACCEPT, CONTENT_TYPE};
use reqwest::{Client as HttpClient, Url};
use sse_stream::SseStream;

use rmcp::model::{ClientJsonRpcMessage, ServerJsonRpcMessage};

/// Header used by the Streamable HTTP transport to carry the session id.
/// Matches `rmcp::transport::streamable_http_server::session::HEADER_SESSION_ID`.
const HEADER_SESSION_ID: &str = "Mcp-Session-Id";
const CONTENT_TYPE_JSON: &str = "application/json";
const CONTENT_TYPE_SSE: &str = "text/event-stream";

/// Number of in-flight POSTs allowed before the `Sink` applies backpressure.
const REQUEST_QUEUE_SIZE: usize = 16;

/// Error type for the Streamable HTTP client transport. Must be
/// `std::error::Error + Send + 'static` to satisfy rmcp's `IntoTransport`.
#[derive(Debug)]
pub struct StreamableHttpError(pub String);

impl fmt::Display for StreamableHttpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "streamable http transport error: {}", self.0)
    }
}

impl std::error::Error for StreamableHttpError {}

// rmcp's `serve()` requires the transport's error type to be convertible from
// `std::io::Error` (it surfaces transport-layer IO failures through this type).
impl From<std::io::Error> for StreamableHttpError {
    fn from(e: std::io::Error) -> Self {
        StreamableHttpError(format!("io error: {}", e))
    }
}

/// A client transport for the MCP Streamable HTTP protocol.
///
/// Outgoing messages are POSTed (one tokio task per message). Server responses
/// can arrive on two channels, both of which feed the internal `incoming`
/// channel that the `Stream` side drains:
///   * the POST request's own response body (`application/json` for the
///     initialize result, or `text/event-stream` for spec-compliant servers
///     that answer requests inline), and
///   * a long-lived GET "common channel" SSE stream, opened once the session
///     id is known. The rmcp 0.1.5 server (and others) deliver request
///     responses here rather than on the POST body, so opening it is required
///     for `tools/list` / `tools/call` to complete.
#[derive(Debug)]
pub struct StreamableHttpClientTransport {
    http_client: HttpClient,
    url: Arc<Url>,
    extra_headers: Arc<HeaderMap>,
    /// Session id assigned by the server on the initialize response, echoed on
    /// every subsequent POST. Shared with the spawned POST tasks. Locks are
    /// only ever held briefly (clone/store), never across an await point.
    session_id: Arc<Mutex<Option<String>>>,
    /// Guards opening the GET common-channel stream exactly once, the first
    /// time a session id becomes available.
    common_channel_open: Arc<AtomicBool>,
    incoming_tx: mpsc::UnboundedSender<ServerJsonRpcMessage>,
    incoming_rx: mpsc::UnboundedReceiver<ServerJsonRpcMessage>,
    /// Completion signals for in-flight POSTs, used for `Sink` backpressure
    /// and flush. Each resolves once the POST's response headers are handled.
    request_queue: VecDeque<oneshot::Receiver<Result<(), StreamableHttpError>>>,
}

impl StreamableHttpClientTransport {
    /// Build a transport targeting `url`, sending `headers` (e.g. an
    /// `Authorization` header) on every request. No network round-trip happens
    /// here — the MCP handshake is driven later by `serve()`.
    pub fn start(
        url: &str,
        headers: &HashMap<String, String>,
    ) -> Result<Self, StreamableHttpError> {
        let parsed = Url::parse(url)
            .map_err(|e| StreamableHttpError(format!("invalid url '{}': {}", url, e)))?;

        let mut header_map = HeaderMap::new();
        for (k, v) in headers {
            let name = HeaderName::from_bytes(k.as_bytes()).map_err(|e| {
                StreamableHttpError(format!("invalid header name '{}': {}", k, e))
            })?;
            let value = HeaderValue::from_str(v).map_err(|e| {
                StreamableHttpError(format!("invalid header value for '{}': {}", k, e))
            })?;
            header_map.insert(name, value);
        }

        let http_client = HttpClient::builder()
            .build()
            .map_err(|e| StreamableHttpError(format!("failed to build http client: {}", e)))?;

        let (incoming_tx, incoming_rx) = mpsc::unbounded();
        Ok(Self {
            http_client,
            url: Arc::new(parsed),
            extra_headers: Arc::new(header_map),
            session_id: Arc::new(Mutex::new(None)),
            common_channel_open: Arc::new(AtomicBool::new(false)),
            incoming_tx,
            incoming_rx,
            request_queue: VecDeque::new(),
        })
    }
}

impl Stream for StreamableHttpClientTransport {
    type Item = ServerJsonRpcMessage;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.get_mut().incoming_rx.poll_next_unpin(cx)
    }
}

impl Sink<ClientJsonRpcMessage> for StreamableHttpClientTransport {
    type Error = StreamableHttpError;

    fn poll_ready(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let this = self.get_mut();
        if this.request_queue.len() >= REQUEST_QUEUE_SIZE {
            let front = this
                .request_queue
                .front_mut()
                .expect("queue is non-empty");
            let result = ready!(front.poll_unpin(cx));
            this.request_queue.pop_front();
            // A `Canceled` error means the POST task was dropped before
            // signalling; treat it as a no-op rather than failing the sink.
            if let Ok(res) = result {
                res?;
            }
        }
        Poll::Ready(Ok(()))
    }

    fn start_send(self: Pin<&mut Self>, item: ClientJsonRpcMessage) -> Result<(), Self::Error> {
        let this = self.get_mut();
        let (done_tx, done_rx) = oneshot::channel();
        let ctx = PostContext {
            client: this.http_client.clone(),
            url: this.url.clone(),
            headers: this.extra_headers.clone(),
            session_id: this.session_id.clone(),
            common_channel_open: this.common_channel_open.clone(),
            incoming_tx: this.incoming_tx.clone(),
        };
        tokio::spawn(async move {
            post_and_forward(ctx, item, done_tx).await;
        });
        this.request_queue.push_back(done_rx);
        Ok(())
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let this = self.get_mut();
        while let Some(front) = this.request_queue.front_mut() {
            let result = ready!(front.poll_unpin(cx));
            this.request_queue.pop_front();
            if let Ok(res) = result {
                res?;
            }
        }
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.poll_flush(cx)
    }
}

/// Shared state captured by each spawned POST task.
#[derive(Clone)]
struct PostContext {
    client: HttpClient,
    url: Arc<Url>,
    headers: Arc<HeaderMap>,
    session_id: Arc<Mutex<Option<String>>>,
    common_channel_open: Arc<AtomicBool>,
    incoming_tx: mpsc::UnboundedSender<ServerJsonRpcMessage>,
}

/// Parse an SSE `data:` payload as a server JSON-RPC message and forward it.
/// Returns `false` if the receiver has been dropped (transport shutting down).
fn forward_sse_data(data: &str, incoming_tx: &mpsc::UnboundedSender<ServerJsonRpcMessage>) -> bool {
    match serde_json::from_str::<ServerJsonRpcMessage>(data) {
        Ok(msg) => incoming_tx.unbounded_send(msg).is_ok(),
        Err(e) => {
            tracing::error!(error = %e, "streamable http: failed to parse SSE JSON-RPC message");
            true
        }
    }
}

/// POST a single client message and forward any response message(s) into the
/// incoming channel. `done` is signalled once the response status/headers are
/// resolved (before draining a possibly long-lived SSE body), so the `Sink`
/// can make progress while responses stream in via the `Stream` side.
async fn post_and_forward(
    ctx: PostContext,
    message: ClientJsonRpcMessage,
    done: oneshot::Sender<Result<(), StreamableHttpError>>,
) {
    let mut request = ctx
        .client
        .post(ctx.url.as_ref().clone())
        .headers(ctx.headers.as_ref().clone())
        .header(ACCEPT, format!("{}, {}", CONTENT_TYPE_JSON, CONTENT_TYPE_SSE));
    if let Some(sid) = ctx.session_id.lock().expect("session_id mutex poisoned").clone() {
        request = request.header(HEADER_SESSION_ID, sid);
    }

    let response = match request
        .json(&message)
        .send()
        .await
        .and_then(|r| r.error_for_status())
    {
        Ok(r) => r,
        Err(e) => {
            let _ = done.send(Err(StreamableHttpError(format!("POST failed: {}", e))));
            return;
        }
    };

    // Capture/refresh the session id (set by the server on initialize), then
    // open the GET common channel the first time we learn it.
    if let Some(value) = response.headers().get(HEADER_SESSION_ID) {
        if let Ok(s) = value.to_str() {
            *ctx.session_id.lock().expect("session_id mutex poisoned") = Some(s.to_string());
            maybe_open_common_channel(&ctx, s.to_string());
        }
    }

    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .unwrap_or_default();

    // The POST succeeded; unblock the Sink before draining the body, which for
    // an SSE response may stay open for the duration of a long-running call.
    let _ = done.send(Ok(()));

    if content_type.starts_with(CONTENT_TYPE_SSE) {
        // Spec-compliant servers may answer the request inline on this stream.
        // Servers that instead use the common channel (e.g. rmcp 0.1.5) leave
        // it empty; we simply receive nothing here and rely on the GET stream.
        let mut events = SseStream::from_byte_stream(response.bytes_stream());
        while let Some(event) = events.next().await {
            match event {
                Ok(sse) => {
                    let Some(data) = sse.data else { continue };
                    if !forward_sse_data(&data, &ctx.incoming_tx) {
                        break;
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, "streamable http: SSE stream error");
                    break;
                }
            }
        }
    } else if content_type.starts_with(CONTENT_TYPE_JSON) {
        match response.bytes().await {
            Ok(body) => match serde_json::from_slice::<ServerJsonRpcMessage>(&body) {
                Ok(msg) => {
                    let _ = ctx.incoming_tx.unbounded_send(msg);
                }
                Err(e) => tracing::error!(
                    error = %e,
                    "streamable http: failed to parse JSON-RPC response"
                ),
            },
            Err(e) => tracing::error!(
                error = %e,
                "streamable http: failed to read JSON-RPC response body"
            ),
        }
    }
    // Otherwise (e.g. 202 Accepted with an empty body): nothing to forward.
}

/// Open the long-lived GET "common channel" SSE stream exactly once, the first
/// time a session id is known. Server-initiated messages and (for rmcp-style
/// servers) request responses are delivered here.
fn maybe_open_common_channel(ctx: &PostContext, session_id: String) {
    if ctx
        .common_channel_open
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return; // already opened
    }
    let client = ctx.client.clone();
    let url = ctx.url.as_ref().clone();
    let headers = ctx.headers.as_ref().clone();
    let incoming_tx = ctx.incoming_tx.clone();
    tokio::spawn(async move {
        let request = client
            .get(url)
            .headers(headers)
            .header(ACCEPT, CONTENT_TYPE_SSE)
            .header(HEADER_SESSION_ID, session_id);
        let response = match request.send().await.and_then(|r| r.error_for_status()) {
            Ok(r) => r,
            Err(e) => {
                // A server that does not offer a GET stream (e.g. 405) is fine:
                // such servers answer requests inline on the POST stream.
                tracing::debug!(error = %e, "streamable http: GET common channel unavailable");
                return;
            }
        };
        let mut events = SseStream::from_byte_stream(response.bytes_stream());
        while let Some(event) = events.next().await {
            match event {
                Ok(sse) => {
                    let Some(data) = sse.data else { continue };
                    if !forward_sse_data(&data, &incoming_tx) {
                        break;
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, "streamable http: common channel SSE error");
                    break;
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_rejects_invalid_url() {
        let err = StreamableHttpClientTransport::start("not a url", &HashMap::new())
            .expect_err("should reject invalid url");
        assert!(err.to_string().contains("invalid url"));
    }

    #[test]
    fn start_rejects_invalid_header_name() {
        let mut headers = HashMap::new();
        headers.insert("inva lid".to_string(), "x".to_string());
        let err = StreamableHttpClientTransport::start("http://localhost:1/mcp", &headers)
            .expect_err("should reject invalid header name");
        assert!(err.to_string().contains("invalid header name"));
    }

    #[test]
    fn start_accepts_valid_url_and_headers() {
        let mut headers = HashMap::new();
        headers.insert("Authorization".to_string(), "Bearer token".to_string());
        let transport =
            StreamableHttpClientTransport::start("http://localhost:8080/mcp", &headers)
                .expect("should build transport");
        assert!(transport.session_id.lock().unwrap().is_none());
        assert_eq!(transport.extra_headers.len(), 1);
    }
}
