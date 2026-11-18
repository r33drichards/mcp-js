//! Minimal S3 HTTP fixture: exercises the real AWS client without Docker or credentials.
use axum::{
    Router,
    body::Bytes,
    extract::{OriginalUri, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Clone, Default)]
pub struct Objects {
    pub values: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    pub fail_put: Arc<AtomicBool>,
}
pub struct S3 {
    pub endpoint: String,
    pub objects: Objects,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for S3 {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl S3 {
    pub async fn start() -> Self {
        async fn put(
            State(objects): State<Objects>,
            OriginalUri(uri): OriginalUri,
            headers: axum::http::HeaderMap,
            body: Bytes,
        ) -> Response {
            if objects.fail_put.load(Ordering::SeqCst) {
                return StatusCode::FORBIDDEN.into_response();
            }
            let mut values = objects.values.lock().unwrap();
            if headers.get("if-none-match").is_some_and(|h| h == "*")
                && values.contains_key(uri.path())
            {
                return StatusCode::PRECONDITION_FAILED.into_response();
            }
            values.insert(uri.path().into(), body.to_vec());
            (StatusCode::OK, [("etag", "\"test\"")]).into_response()
        }
        async fn fetch(State(objects): State<Objects>, OriginalUri(uri): OriginalUri) -> Response {
            match objects.values.lock().unwrap().get(uri.path()).cloned() {
                Some(value) => (StatusCode::OK, value).into_response(),
                None => StatusCode::NOT_FOUND.into_response(),
            }
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let objects = Objects::default();
        let app = Router::new()
            .route("/{*key}", get(fetch).put(put))
            .with_state(objects.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            endpoint,
            objects,
            task,
        }
    }
    pub fn configure(&self, command: &mut tokio::process::Command) {
        command
            .args(["--s3-bucket", "runtime-test"])
            .env("AWS_ENDPOINT_URL", &self.endpoint)
            .env("AWS_S3_FORCE_PATH_STYLE", "true")
            .env("AWS_ACCESS_KEY_ID", "test")
            .env("AWS_SECRET_ACCESS_KEY", "test")
            .env("AWS_REGION", "us-east-1")
            .env("AWS_EC2_METADATA_DISABLED", "true")
            .env("AWS_REQUEST_CHECKSUM_CALCULATION", "WHEN_REQUIRED");
    }
}
