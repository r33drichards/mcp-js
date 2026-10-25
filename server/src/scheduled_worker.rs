//! Dedicated scheduled-only worker router. No legacy execution or MCP fallback.
use crate::scheduled_protocol::{Admission, Dispatch, Payload, Permit};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{future::Future, pin::Pin, sync::Arc};

pub trait Runtime: Send + Sync + 'static {
    fn submit(
        &self,
        payload: Payload,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + '_>>;
    fn status(&self, id: String) -> Result<Value, String>;
    fn cancel(&self, _id: String) -> Result<(), String> {
        Err("runtime does not support cancellation".into())
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub namespace: String,
    pub worker_id: String,
    pub data_dir: String,
    pub bearer_env: String,
    pub admission_bearer_env: String,
    pub admission_urls: Vec<String>,
}
pub fn validate_origin(origin: &str) -> Result<(), String> {
    let url = reqwest::Url::parse(origin).map_err(|e| e.to_string())?;
    if (url.scheme() != "https"
        && !(url.scheme() == "http"
            && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err("require HTTPS origin or loopback HTTP without credentials".into());
    }
    Ok(())
}
pub fn secret(reference: &str) -> Result<String, String> {
    std::env::var(reference)
        .ok()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "credential reference unavailable".into())
}
pub struct Worker {
    config: Config,
    db: sled::Db,
    incarnation: String,
    bearer: String,
    admission_bearer: String,
    client: reqwest::Client,
    execution_gate: tokio::sync::Mutex<()>,
    #[cfg(test)]
    fail_flush: std::sync::atomic::AtomicBool,
}
/// An owned, non-cloneable launch capability. No reconstruction from a receipt.
pub struct Launch {
    payload: Payload,
}
impl Launch {
    pub async fn run(self, runtime: &dyn Runtime) -> Result<String, String> {
        runtime.submit(self.payload).await
    }
}
impl Worker {
    pub fn new(config: Config) -> Result<Arc<Self>, String> {
        if config.namespace.is_empty()
            || config.worker_id.is_empty()
            || config.admission_urls.is_empty()
        {
            return Err("worker identity, namespace and admission endpoints required".into());
        }
        for url in &config.admission_urls {
            validate_origin(url)?;
        }
        let bearer = secret(&config.bearer_env)?;
        let admission_bearer = secret(&config.admission_bearer_env)?;
        let path = std::path::Path::new(&config.data_dir);
        if path.exists()
            && std::fs::read_dir(path)
                .map_err(|e| e.to_string())?
                .next()
                .is_some()
            && !path.join("conf").is_file()
        {
            return Err("worker requires a fresh directory or bound sled store".into());
        }
        let db = sled::open(&config.data_dir).map_err(|e| e.to_string())?;
        let binding = serde_json::to_vec(&(1, &config.namespace, &config.worker_id)).unwrap();
        match db.get("binding").map_err(|e| e.to_string())? {
            Some(old) if old.as_ref() != binding => {
                return Err("worker store identity/version mismatch".into());
            }
            None => {
                for name in db.tree_names() {
                    if !db.open_tree(name).map_err(|e| e.to_string())?.is_empty() {
                        return Err("worker requires a fresh or bound store".into());
                    }
                }
                db.insert("binding", binding).map_err(|e| e.to_string())?;
                db.flush().map_err(|e| e.to_string())?;
            }
            _ => (),
        }
        Ok(Arc::new(Self {
            execution_gate: tokio::sync::Mutex::new(()),
            #[cfg(test)]
            fail_flush: std::sync::atomic::AtomicBool::new(false),
            config,
            db,
            incarnation: format!("{:032x}", rand::random::<u128>()),
            bearer,
            admission_bearer,
            client: reqwest::Client::builder()
                .retry(reqwest::retry::never())
                .redirect(reqwest::redirect::Policy::none())
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .map_err(|e| e.to_string())?,
        }))
    }
    fn flush(&self) -> Result<(), String> {
        #[cfg(test)]
        if self.fail_flush.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("injected worker flush failure".into());
        }
        self.db.flush().map(|_| ()).map_err(|e| e.to_string())
    }
    #[cfg(test)]
    pub(crate) fn inject_flush_failure(&self) {
        self.fail_flush
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
    /// Reserve before the network operation. Any uncertain/cancelled attempt remains
    /// consumed locally across restart, even if it never obtained quorum admission.
    pub fn reserve(&self, dispatch: Dispatch) -> Result<Admission, String> {
        dispatch.validate()?;
        if dispatch.namespace != self.config.namespace
            || dispatch.worker_id != self.config.worker_id
            || Utc::now() >= dispatch.payload.expires_at
        {
            return Err("wrong worker/namespace or expired authority".into());
        }
        let admission = Admission {
            dispatch,
            worker_incarnation: self.incarnation.clone(),
        };
        let key = format!("invocation/{}", admission.dispatch.key());
        let bytes = serde_json::to_vec(&admission).unwrap();
        self.db
            .compare_and_swap(key, None as Option<&[u8]>, Some(bytes))
            .map_err(|e| e.to_string())?
            .map_err(|_| "invocation already reserved; no replay".to_string())?;
        self.db
            .insert(
                format!("dispatch/{}", admission.dispatch.key()),
                serde_json::to_vec(&admission.dispatch).unwrap(),
            )
            .map_err(|e| e.to_string())?;
        self.flush()?;
        Ok(admission)
    }
    pub async fn admit(&self, admission: &Admission) -> Result<Permit, String> {
        for origin in &self.config.admission_urls {
            let response = self
                .client
                .post(format!(
                    "{}/scheduler/v1/admit",
                    origin.trim_end_matches('/')
                ))
                .bearer_auth(&self.admission_bearer)
                .json(admission)
                .send()
                .await
                .map_err(|_| "ambiguous admission response")?;
            if response.status() == reqwest::StatusCode::CONFLICT {
                continue;
            }
            if response.status() != reqwest::StatusCode::CREATED {
                return Err("admission unavailable or uncertain".into());
            }
            let permit: Permit = response.json().await.map_err(|_| "invalid admission ACK")?;
            if permit != admission.permit() {
                return Err("admission ACK binding mismatch".into());
            }
            return Ok(permit);
        }
        Err("no current owner admitted invocation".into())
    }
    pub fn consume(&self, admission: Admission, permit: Permit) -> Result<Launch, String> {
        if self
            .db
            .contains_key(format!("cancel/{}", admission.dispatch.key()))
            .map_err(|e| e.to_string())?
        {
            return Err("invocation cancelled before launch".into());
        }
        if permit != admission.permit() || admission.worker_incarnation != self.incarnation {
            return Err("wrong launch binding/incarnation".into());
        }
        let key = format!("invocation/{}", admission.dispatch.key());
        let reservation = serde_json::to_vec(&admission).unwrap();
        self.db
            .compare_and_swap(key, Some(reservation), Some(b"consumed".as_slice()))
            .map_err(|e| e.to_string())?
            .map_err(|_| "launch permission already consumed".to_string())?;
        self.flush()?;
        Ok(Launch {
            payload: admission.dispatch.payload,
        })
    }
    pub fn router(self: Arc<Self>, runtime: Arc<dyn Runtime>) -> Router {
        Router::new()
            .route("/api/scheduled/v1/exec", post(execute))
            .route("/api/scheduled/v1/executions/{id}", get(status))
            .route("/api/scheduled/v1/cancel", post(cancel))
            .layer(axum::extract::DefaultBodyLimit::max(16 * 1024 * 1024))
            .with_state((self, runtime))
    }
    fn authorized(&self, headers: &HeaderMap) -> bool {
        headers
            .get("authorization")
            .and_then(|h| h.to_str().ok())
            .is_some_and(|h| h == format!("Bearer {}", self.bearer))
    }
}
type Shared = (Arc<Worker>, Arc<dyn Runtime>);
async fn execute(
    State((worker, runtime)): State<Shared>,
    headers: HeaderMap,
    Json(dispatch): Json<Dispatch>,
) -> (StatusCode, Json<Value>) {
    if !worker.authorized(&headers) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"unauthorized"})),
        );
    }
    let result = async {
        let admission = worker.reserve(dispatch)?;
        let permit = worker.admit(&admission).await?;
        // Serializes cancellation with the entire consume -> submit -> receipt
        // boundary. A cancelled reservation can never expose a late launch.
        let _gate = worker.execution_gate.lock().await;
        let key = admission.dispatch.key();
        let launch = worker.consume(admission, permit)?;
        let execution_id = launch.run(runtime.as_ref()).await?;
        worker
            .db
            .insert(
                format!("execution/{key}"),
                serde_json::to_vec(&(&worker.incarnation, &execution_id)).unwrap(),
            )
            .map_err(|e| e.to_string())?;
        worker.flush()?;
        Ok::<_, String>(execution_id)
    }
    .await;
    match result {
        Ok(execution_id) => (
            StatusCode::ACCEPTED,
            Json(json!({"execution_id":execution_id})),
        ),
        Err(error) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error":error})),
        ),
    }
}

async fn cancel(
    State((worker, runtime)): State<Shared>,
    headers: HeaderMap,
    Json(dispatch): Json<Dispatch>,
) -> (StatusCode, Json<Value>) {
    if !worker.authorized(&headers) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"unauthorized"})),
        );
    }
    let result = async {
        dispatch.validate()?;
        if dispatch.namespace != worker.config.namespace
            || dispatch.worker_id != worker.config.worker_id
        {
            return Err("cancellation worker binding mismatch".to_string());
        }
        let _gate = worker.execution_gate.lock().await;
        let key = dispatch.key();
        if let Some(bound) = worker
            .db
            .get(format!("dispatch/{key}"))
            .map_err(|e| e.to_string())?
            && bound.as_ref() != serde_json::to_vec(&dispatch).unwrap()
        {
            return Err("cancellation dispatch binding mismatch".into());
        }
        // Durable intent is retained even if a runtime/status call fails.
        worker
            .db
            .insert(format!("cancel/{key}"), b"cancelled")
            .map_err(|e| e.to_string())?;
        worker.flush()?;
        if let Some(receipt) = worker
            .db
            .get(format!("execution/{key}"))
            .map_err(|e| e.to_string())?
        {
            let (incarnation, id): (String, String) =
                serde_json::from_slice(&receipt).map_err(|e| e.to_string())?;
            if incarnation != worker.incarnation {
                return Err("previous worker execution outcome remains unknown".into());
            }
            // Completion may have won. The runtime status below is authoritative;
            // a cancellation request/acknowledgement alone never releases capacity.
            let _ = runtime.cancel(id.clone());
            let status = runtime.status(id)?;
            if status.get("stopped").and_then(Value::as_bool) != Some(true) {
                return Ok(json!({"status":"cancelling"}));
            }
            return Ok(status);
        }
        let record = worker
            .db
            .get(format!("invocation/{key}"))
            .map_err(|e| e.to_string())?;
        if record.is_some_and(|r| r.as_ref() == b"consumed") {
            return Err(
                "launch consumed without durable execution receipt; outcome unknown".into(),
            );
        }
        Ok(json!({"status":"cancelled", "stopped":true}))
    }
    .await;
    match result {
        Ok(status) => (StatusCode::OK, Json(status)),
        Err(error) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error":error})),
        ),
    }
}
async fn status(
    State((worker, runtime)): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> (StatusCode, Json<Value>) {
    if !worker.authorized(&headers) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"unauthorized"})),
        );
    }
    match runtime.status(id) {
        Ok(value) => (StatusCode::OK, Json(value)),
        Err(error) => (StatusCode::NOT_FOUND, Json(json!({"error":error}))),
    }
}

/// Load only an operator-supplied config. No implicit fallback if it is invalid.
pub fn configured() -> bool {
    std::env::var_os("MCP_SCHEDULED_WORKER_CONFIG").is_some()
}
pub fn from_environment() -> Result<Arc<Worker>, String> {
    let path = std::env::var_os("MCP_SCHEDULED_WORKER_CONFIG").ok_or("worker config missing")?;
    let config = serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    Worker::new(config)
}
