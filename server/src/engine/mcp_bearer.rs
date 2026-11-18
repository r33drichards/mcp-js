//! File-reference bearer authentication for one downstream HTTP MCP server.
//!
//! The config contains only a path. Secret contents live in a sensitive header
//! inside a dedicated client, never in rmcp's Debug-printable transport config.

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use http::{HeaderName, HeaderValue, header::AUTHORIZATION};
use rmcp::model::ClientJsonRpcMessage;
use rmcp::transport::common::client_side_sse::BoxedSseResponse;
use rmcp::transport::streamable_http_client::{
    SseError, StreamableHttpClient, StreamableHttpError, StreamableHttpPostResponse,
};
use tracing::instrument::WithSubscriber;

const MAX_TOKEN_BYTES: u64 = 64 * 1024;
const DEFAULT_MAX_SSE_EVENT_SIZE: usize = 16 * 1024 * 1024;
type TransportError = StreamableHttpError<reqwest_mcp::Error>;

fn failure(server_name: &str, detail: &str) -> String {
    format!("MCP server '{server_name}': bearer auth {detail}")
}

pub(super) fn validate_reference(
    server_name: &str,
    uri: &str,
    token_file: &str,
) -> Result<(), String> {
    if !Path::new(token_file).is_absolute() {
        return Err(failure(
            server_name,
            "token_file must be an absolute file path",
        ));
    }
    let url =
        url::Url::parse(uri).map_err(|_| failure(server_name, "requires a valid HTTP(S) URL"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(failure(
            server_name,
            "requires an HTTP(S) URL without userinfo or a fragment",
        ));
    }
    Ok(())
}

fn read_authorization(server_name: &str, token_file: &str) -> Result<HeaderValue, String> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    // Follow Kubernetes projected-secret symlinks, but never block opening a
    // FIFO or device masquerading as a token file. Validate the opened handle.
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options
        .open(token_file)
        .map_err(|_| failure(server_name, "token_file could not be read"))?;
    if !file
        .metadata()
        .map_err(|_| failure(server_name, "token_file metadata could not be read"))?
        .is_file()
    {
        return Err(failure(server_name, "token_file must be a regular file"));
    }
    let mut bytes = Vec::new();
    file.take(MAX_TOKEN_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| failure(server_name, "token_file could not be read"))?;
    if bytes.len() as u64 > MAX_TOKEN_BYTES {
        return Err(failure(server_name, "token_file exceeds 64 KiB"));
    }
    // Permit one text-file line ending, not arbitrary whitespace or multiple
    // lines. RFC 6750 b64token validation also rejects header injection.
    let token = std::str::from_utf8(&bytes)
        .map_err(|_| failure(server_name, "token_file is not a valid bearer token"))?;
    let token = token
        .strip_suffix("\r\n")
        .or_else(|| token.strip_suffix('\n'))
        .unwrap_or(token);
    let value = token.trim_end_matches('=');
    if value.is_empty()
        || !value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'+' | b'/')
        })
    {
        return Err(failure(
            server_name,
            "token_file is empty or is not a valid bearer token",
        ));
    }
    let mut header = HeaderValue::from_str(&format!("Bearer {token}"))
        .map_err(|_| failure(server_name, "token_file is not a valid bearer token"))?;
    header.set_sensitive(true);
    Ok(header)
}

// This private wrapper never participates in configuration serialization.
#[derive(Clone)]
struct SecretToken(Arc<str>);

impl std::fmt::Debug for SecretToken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

impl SecretToken {
    fn redact_json(&self, value: &mut serde_json::Value) {
        match value {
            serde_json::Value::String(value) => {
                *value = value.replace(self.0.as_ref(), "[REDACTED]")
            }
            serde_json::Value::Array(values) => {
                for value in values {
                    self.redact_json(value);
                }
            }
            serde_json::Value::Object(values) => {
                *values = std::mem::take(values)
                    .into_iter()
                    .map(|(key, mut value)| {
                        self.redact_json(&mut value);
                        (key.replace(self.0.as_ref(), "[REDACTED]"), value)
                    })
                    .collect();
            }
            _ => {}
        }
    }

    fn check_session(&self, session: &Option<String>) -> Result<(), TransportError> {
        if session
            .as_ref()
            .is_some_and(|value| value.contains(self.0.as_ref()))
        {
            return Err(redacted_error());
        }
        Ok(())
    }

    fn stream(&self, stream: BoxedSseResponse) -> BoxedSseResponse {
        let secret = self.clone();
        let mut stream = stream;
        // Keep raw SSE parser diagnostics disabled even if another dependency
        // enables sse-stream's optional tracing feature in the future.
        futures::stream::poll_fn(move |cx| {
            tracing::subscriber::with_default(tracing::subscriber::NoSubscriber::default(), || {
                stream.as_mut().poll_next(cx)
            })
        })
        .map(move |event| {
            let failure = || {
                SseError::Body(
                    std::io::Error::other(
                        "bearer-authenticated SSE response rejected (details redacted)",
                    )
                    .into(),
                )
            };
            let mut event = event.map_err(|_| failure())?;
            // IDs participate in resume requests, so fail rather than rewrite.
            if event
                .id
                .as_ref()
                .is_some_and(|value| value.contains(secret.0.as_ref()))
                || event
                    .event
                    .as_ref()
                    .is_some_and(|value| value.contains(secret.0.as_ref()))
            {
                return Err(failure());
            }
            if let Some(data) = &mut event.data {
                if !data.trim().is_empty() {
                    let mut value: serde_json::Value =
                        serde_json::from_str(data).map_err(|_| failure())?;
                    secret.redact_json(&mut value);
                    *data = serde_json::to_string(&value).map_err(|_| failure())?;
                }
            }
            Ok(event)
        })
        .boxed()
    }

    fn response(
        &self,
        response: StreamableHttpPostResponse,
    ) -> Result<StreamableHttpPostResponse, TransportError> {
        match response {
            StreamableHttpPostResponse::Accepted => Ok(StreamableHttpPostResponse::Accepted),
            StreamableHttpPostResponse::Json(message, session) => {
                self.check_session(&session)?;
                let mut value = serde_json::to_value(message).map_err(|_| redacted_error())?;
                self.redact_json(&mut value);
                let message = serde_json::from_value(value).map_err(|_| redacted_error())?;
                Ok(StreamableHttpPostResponse::Json(message, session))
            }
            StreamableHttpPostResponse::Sse(stream, session) => {
                self.check_session(&session)?;
                Ok(StreamableHttpPostResponse::Sse(
                    self.stream(stream),
                    session,
                ))
            }
            _ => Err(redacted_error()),
        }
    }
}

fn redacted_error() -> TransportError {
    TransportError::UnexpectedServerResponse(
        "bearer-authenticated HTTP request failed (details redacted)".into(),
    )
}

#[derive(Clone)]
pub(super) struct BearerHttpClient {
    client: reqwest_mcp::Client,
    uri: Arc<str>,
    secret: SecretToken,
}

impl std::fmt::Debug for BearerHttpClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BearerHttpClient")
            .finish_non_exhaustive()
    }
}

impl BearerHttpClient {
    pub(super) fn from_file(
        server_name: &str,
        uri: &str,
        token_file: &str,
    ) -> Result<Self, String> {
        validate_reference(server_name, uri, token_file)?;
        let mut headers = http::HeaderMap::new();
        let authorization = read_authorization(server_name, token_file)?;
        let token = authorization
            .to_str()
            .map_err(|_| failure(server_name, "invalid token header"))?
            .strip_prefix("Bearer ")
            .ok_or_else(|| failure(server_name, "invalid token header"))?;
        let secret = SecretToken(Arc::from(token));
        if uri.contains(secret.0.as_ref()) {
            return Err(failure(
                server_name,
                "token must not appear in the endpoint URL",
            ));
        }
        headers.insert(AUTHORIZATION, authorization);
        let client = reqwest_mcp::Client::builder()
            .default_headers(headers)
            // Disable even same-origin redirects: a token is scoped to this
            // exact configured endpoint, including GET/POST/session cleanup.
            .redirect(reqwest_mcp::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(30))
            .build()
            .map_err(|_| failure(server_name, "HTTP client setup failed"))?;
        Ok(Self {
            client,
            uri: uri.into(),
            secret,
        })
    }

    fn check_target(
        &self,
        uri: &str,
        auth_header: &Option<String>,
        custom_headers: &HashMap<HeaderName, HeaderValue>,
    ) -> Result<(), TransportError> {
        if uri != self.uri.as_ref()
            || auth_header.is_some()
            || custom_headers.contains_key(&AUTHORIZATION)
            || custom_headers.contains_key(&http::header::HOST)
        {
            return Err(TransportError::UnexpectedServerResponse(
                "bearer auth request target or auth override rejected".into(),
            ));
        }
        Ok(())
    }
}

// rmcp's HTTP errors can include response bodies and challenges, which may
// echo credentials. Preserve only control-flow variants, never remote text.
fn safe_error(error: TransportError) -> TransportError {
    match error {
        TransportError::ServerDoesNotSupportSse => TransportError::ServerDoesNotSupportSse,
        TransportError::ServerDoesNotSupportDeleteSession => {
            TransportError::ServerDoesNotSupportDeleteSession
        }
        TransportError::SessionExpired => TransportError::SessionExpired,
        _ => redacted_error(),
    }
}

impl StreamableHttpClient for BearerHttpClient {
    type Error = reqwest_mcp::Error;

    async fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<StreamableHttpPostResponse, TransportError> {
        self.post_message_with_max_sse_event_size(
            uri,
            message,
            session_id,
            auth_header,
            custom_headers,
            DEFAULT_MAX_SSE_EVENT_SIZE,
        )
        .await
    }

    async fn post_message_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
        max_sse_event_size: usize,
    ) -> Result<StreamableHttpPostResponse, TransportError> {
        self.check_target(&uri, &auth_header, &custom_headers)?;
        // The upstream adapter logs remote headers before returning errors.
        // Silence only this future; sanitized results retain our normal logs.
        let response = self
            .client
            .post_message_with_max_sse_event_size(
                uri,
                message,
                session_id,
                None,
                custom_headers,
                max_sse_event_size,
            )
            .with_subscriber(tracing::subscriber::NoSubscriber::default())
            .await
            .map_err(safe_error)?;
        self.secret.response(response)
    }

    async fn delete_session(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<(), TransportError> {
        self.check_target(&uri, &auth_header, &custom_headers)?;
        self.client
            .delete_session(uri, session_id, None, custom_headers)
            .with_subscriber(tracing::subscriber::NoSubscriber::default())
            .await
            .map_err(safe_error)
    }

    async fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<BoxedSseResponse, TransportError> {
        self.get_stream_with_max_sse_event_size(
            uri,
            session_id,
            last_event_id,
            auth_header,
            custom_headers,
            DEFAULT_MAX_SSE_EVENT_SIZE,
        )
        .await
    }

    async fn get_stream_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
        max_sse_event_size: usize,
    ) -> Result<BoxedSseResponse, TransportError> {
        self.check_target(&uri, &auth_header, &custom_headers)?;
        let stream = self
            .client
            .get_stream_with_max_sse_event_size(
                uri,
                session_id,
                last_event_id,
                None,
                custom_headers,
                max_sse_event_size,
            )
            .with_subscriber(tracing::subscriber::NoSubscriber::default())
            .await
            .map_err(safe_error)?;
        Ok(self.secret.stream(stream))
    }
}

#[cfg(test)]
#[path = "mcp_bearer_tests.rs"]
mod tests;
