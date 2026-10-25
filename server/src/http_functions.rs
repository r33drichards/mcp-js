//! Dynamic HTTP dispatch through the ordinary Engine, never the scheduler worker.
//! Authentication is applied by the transport, exactly as for /api/exec.
use crate::{
    cluster::ClusterNode,
    engine::{Engine, ExecutionRequest},
};
use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::{Path, Request, State},
    http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use tokio::sync::{Mutex, Semaphore};
use tracing::Instrument;

const KEY: &str = "http-functions/v1/registry";
const MAX_BODY: usize = 1024 * 1024;
const MAX_SOURCE: usize = 64 * 1024;
const MAX_DEFINITIONS: usize = 32;
type Failure = StatusCode;

#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    pub timeout_seconds: u64,
    pub heap_mb: u64,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            timeout_seconds: 30,
            heap_mb: 64,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Definition {
    pub version: u32,
    pub methods: Vec<String>,
    pub source: String,
    pub export: String,
    #[serde(default)]
    pub limits: Limits,
}
#[derive(Clone, Serialize, Deserialize)]
struct Entry {
    revision: String,
    definition: Option<Definition>,
}
type Records = BTreeMap<String, Entry>;
enum Backend {
    Local(sled::Tree),
    Cluster(Arc<ClusterNode>),
}
struct Registry {
    backend: Backend,
    lock: Mutex<()>,
}
impl Registry {
    async fn load(&self) -> Result<(Option<String>, Records), Failure> {
        let raw = match &self.backend {
            Backend::Local(tree) => tree
                .get(KEY)
                .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
                .map(|v| String::from_utf8(v.to_vec()).map_err(|_| StatusCode::SERVICE_UNAVAILABLE))
                .transpose()?,
            Backend::Cluster(node) => {
                // Commit a current-term entry before reading the CAS candidate.
                // Otherwise an uncommitted prior-term registry suffix can keep
                // disagreeing with the applied value after restart forever.
                node.put("http-functions/v1/barrier".into(), String::new())
                    .await
                    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
                node.get(KEY)
                    .await
                    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
            }
        };
        let records = match &raw {
            Some(raw) => serde_json::from_str(raw).map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?,
            None => Records::new(),
        };
        if records.len() > MAX_DEFINITIONS
            || records.iter().any(|(name, entry)| {
                !valid_name(name)
                    || uuid::Uuid::parse_str(&entry.revision).is_err()
                    || entry
                        .definition
                        .as_ref()
                        .is_some_and(|def| validate(def).is_err())
            })
        {
            return Err(StatusCode::SERVICE_UNAVAILABLE);
        }
        Ok((raw, records))
    }
    async fn cas(&self, old: Option<String>, records: &Records) -> Result<bool, Failure> {
        let new = serde_json::to_string(records).map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        match &self.backend {
            Backend::Local(tree) => {
                let changed = tree
                    .compare_and_swap(KEY, old.as_deref().map(str::as_bytes), Some(new.as_bytes()))
                    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
                    .is_ok();
                if changed {
                    tree.flush_async()
                        .await
                        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
                }
                Ok(changed)
            }
            Backend::Cluster(node) => node
                .cas(KEY.into(), old, new)
                .await
                .map_err(|_| StatusCode::SERVICE_UNAVAILABLE),
        }
    }
    // Committed compare-and-set is also a read barrier. Leader-only: a follower
    // or a node without quorum fails closed rather than using its local cache.
    async fn read(&self) -> Result<Records, Failure> {
        let _guard = self.lock.lock().await;
        let (old, records) = self.load().await?;
        if matches!(self.backend, Backend::Cluster(_)) && !self.cas(old, &records).await? {
            return Err(StatusCode::SERVICE_UNAVAILABLE);
        }
        Ok(records)
    }
    async fn change(
        &self,
        name: String,
        headers: &HeaderMap,
        definition: Option<Definition>,
    ) -> Result<Entry, Failure> {
        let _guard = self.lock.lock().await;
        let (old, mut records) = self.load().await?;
        let existing = records.get(&name);
        let create = headers.get("if-none-match").and_then(|v| v.to_str().ok()) == Some("*");
        let matched = headers.get("if-match").and_then(|v| v.to_str().ok());
        if create && matched.is_some() {
            return Err(StatusCode::BAD_REQUEST);
        }
        if !create && matched.is_none() {
            return Err(StatusCode::PRECONDITION_REQUIRED);
        }
        let active = existing.is_some_and(|entry| entry.definition.is_some());
        if create {
            if active || definition.is_none() {
                return Err(StatusCode::PRECONDITION_FAILED);
            }
        } else if !active
            || existing
                .map(|entry| format!("\"{}\"", entry.revision))
                .as_deref()
                != matched
        {
            return Err(StatusCode::PRECONDITION_FAILED);
        }
        // Tombstones are retained and count against the bound; no unsafe GC.
        if existing.is_none() && records.len() >= MAX_DEFINITIONS {
            return Err(StatusCode::CONFLICT);
        }
        let entry = Entry {
            revision: uuid::Uuid::new_v4().to_string(),
            definition,
        };
        records.insert(name, entry.clone());
        if !self.cas(old, &records).await? {
            return Err(StatusCode::PRECONDITION_FAILED);
        }
        Ok(entry)
    }
}
#[derive(Clone)]
struct App {
    engine: Arc<Engine>,
    registry: Arc<Registry>,
    slots: Arc<Semaphore>,
}
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        && !["api", "mcp", "health", "scheduler", "discovery"].contains(&name)
}
fn valid_export(name: &str) -> bool {
    let mut chars = name.bytes();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == b'_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == b'_')
        && name != "__mcp_http_result"
}
fn validate(def: &Definition) -> Result<(), Failure> {
    if def.version != 1
        || def.source.len() > MAX_SOURCE
        || def.source.is_empty()
        || !valid_export(&def.export)
        || def.methods.is_empty()
        || def.methods.len() > 8
        || def.limits.timeout_seconds == 0
        || def.limits.timeout_seconds > 30
        || !(8..=64).contains(&def.limits.heap_mb)
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    for method in &def.methods {
        if !["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"].contains(&method.as_str())
        {
            return Err(StatusCode::BAD_REQUEST);
        }
    }
    // Parser/transpiler only: never execute registration source.
    handler_code(def, &serde_json::Value::Null).map_err(|_| StatusCode::BAD_REQUEST)?;
    Ok(())
}
fn handler_code(def: &Definition, input: &serde_json::Value) -> Result<String, String> {
    crate::engine::strip_typescript_types(&format!(
        "{source}\n; if (typeof {export} !== 'function') throw new Error('invalid export'); export const __mcp_http_result = JSON.stringify(await {export}({input}));",
        source = def.source,
        export = def.export,
    ))
}
fn metadata(name: &str, entry: &Entry) -> serde_json::Value {
    let def = entry.definition.as_ref().expect("active entry");
    serde_json::json!({"name":name,"revision":entry.revision,"version":1,"methods":def.methods,"limits":def.limits})
}
fn entry_response(name: &str, entry: &Entry) -> Response {
    let mut response = Json(if entry.definition.is_some() {
        metadata(name, entry)
    } else {
        serde_json::json!({"revision":entry.revision,"deleted":true})
    })
    .into_response();
    response.headers_mut().insert(
        "etag",
        HeaderValue::from_str(&format!("\"{}\"", entry.revision)).unwrap(),
    );
    response
}
#[utoipa::path(get, path = "/api/http-functions/v1/definitions", tag = "HTTP functions",
    responses((status = 200, description = "Active definition metadata; source omitted"), (status = 503, description = "Registry unavailable")))]
async fn list(State(app): State<App>) -> Result<Json<Vec<serde_json::Value>>, Failure> {
    let records = app.registry.read().await?;
    Ok(Json(
        records
            .iter()
            .filter(|(_, e)| e.definition.is_some())
            .map(|(n, e)| metadata(n, e))
            .collect(),
    ))
}
#[utoipa::path(get, path = "/api/http-functions/v1/definitions/{name}", tag = "HTTP functions",
    params(("name" = String, Path, description = "Function name")),
    responses((status = 200, description = "Metadata with revision ETag"), (status = 404, description = "Missing function")))]
async fn lookup(State(app): State<App>, Path(name): Path<String>) -> Result<Response, Failure> {
    if !valid_name(&name) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let records = app.registry.read().await?;
    let entry = records
        .get(&name)
        .filter(|e| e.definition.is_some())
        .ok_or(StatusCode::NOT_FOUND)?;
    Ok(entry_response(&name, entry))
}
#[utoipa::path(put, path = "/api/http-functions/v1/definitions/{name}", tag = "HTTP functions",
    params(("name" = String, Path, description = "Function name")), request_body = Definition,
    responses((status = 200, description = "Committed definition with ETag"), (status = 412, description = "Stale revision"), (status = 428, description = "Conditional header required")))]
async fn put(
    State(app): State<App>,
    Path(name): Path<String>,
    request: Request,
) -> Result<Response, Failure> {
    if !valid_name(&name) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let (parts, body) = request.into_parts();
    let bytes = to_bytes(body, MAX_SOURCE + 4096)
        .await
        .map_err(|_| StatusCode::PAYLOAD_TOO_LARGE)?;
    let def: Definition = serde_json::from_slice(&bytes).map_err(|_| StatusCode::BAD_REQUEST)?;
    validate(&def)?;
    let entry = app
        .registry
        .change(name.clone(), &parts.headers, Some(def))
        .await?;
    Ok(entry_response(&name, &entry))
}
#[utoipa::path(delete, path = "/api/http-functions/v1/definitions/{name}", tag = "HTTP functions",
    params(("name" = String, Path, description = "Function name")),
    responses((status = 200, description = "Committed tombstone"), (status = 412, description = "Stale revision")))]
async fn delete(
    State(app): State<App>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Result<Response, Failure> {
    if !valid_name(&name) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let entry = app.registry.change(name.clone(), &headers, None).await?;
    Ok(entry_response(&name, &entry))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HandlerResponse {
    status: u16,
    #[serde(default)]
    headers: BTreeMap<String, String>,
    body: Option<String>,
    body_base64: Option<String>,
}
fn response(value: &str, method: &Method) -> Result<Response, Failure> {
    let fail = StatusCode::INTERNAL_SERVER_ERROR;
    if value.len() > MAX_BODY * 2 {
        return Err(fail);
    }
    let output: HandlerResponse = serde_json::from_str(value).map_err(|_| fail)?;
    if !(200..=599).contains(&output.status) || output.headers.len() > 32 {
        return Err(fail);
    }
    let body = match (output.body, output.body_base64) {
        (Some(text), None) => text.into_bytes(),
        (None, Some(encoded)) => STANDARD.decode(encoded).map_err(|_| fail)?,
        _ => return Err(fail),
    };
    if body.len() > MAX_BODY {
        return Err(fail);
    }
    let bodyless = matches!(output.status, 204 | 205 | 304);
    let mut result = Response::builder().status(output.status);
    let mut total = 0;
    for (name, value) in output.headers {
        total += name.len() + value.len();
        let name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| fail)?;
        if total > 8192
            || [
                "connection",
                "transfer-encoding",
                "content-length",
                "keep-alive",
                "trailer",
                "upgrade",
                "te",
                "proxy-authenticate",
                "proxy-authorization",
            ]
            .contains(&name.as_str())
        {
            return Err(fail);
        }
        result = result.header(name, HeaderValue::from_str(&value).map_err(|_| fail)?);
    }
    if !bodyless {
        result = result.header("content-length", body.len());
    }
    result
        .body(Body::from(if bodyless || *method == Method::HEAD {
            Vec::new()
        } else {
            body
        }))
        .map_err(|_| fail)
}
#[utoipa::path(post, path = "/functions/v1/{name}", tag = "HTTP functions",
    params(("name" = String, Path, description = "Function name; exact registered methods apply")),
    responses((status = 200, description = "Validated handler response; handler chooses status and content type"),
        (status = 404, description = "Missing function"), (status = 405, description = "Method not registered"),
        (status = 409, description = "Invocation capacity exhausted"), (status = 413, description = "Input exceeds limit"),
        (status = 500, description = "Handler or response failed"), (status = 503, description = "Registry or Engine unavailable"),
        (status = 504, description = "Execution timed out")))]
async fn invoke(
    State(app): State<App>,
    Path(name): Path<String>,
    request: Request,
) -> Result<Response, Failure> {
    if !valid_name(&name) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let permit = app
        .slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| StatusCode::CONFLICT)?;
    let entry = app
        .registry
        .read()
        .await?
        .remove(&name)
        .filter(|e| e.definition.is_some())
        .ok_or(StatusCode::NOT_FOUND)?;
    tracing::debug!(function = %name, revision = %entry.revision, "HTTP function revision admitted");
    let def = entry.definition.unwrap();
    validate(&def).map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    if !def.methods.iter().any(|m| m == request.method().as_str()) {
        let mut result = StatusCode::METHOD_NOT_ALLOWED.into_response();
        result.headers_mut().insert(
            "allow",
            HeaderValue::from_str(&def.methods.join(", ")).unwrap(),
        );
        return Ok(result);
    }
    let (parts, body) = request.into_parts();
    if parts.uri.to_string().len() > 8192 {
        return Err(StatusCode::BAD_REQUEST);
    }
    let bytes = to_bytes(body, MAX_BODY)
        .await
        .map_err(|_| StatusCode::PAYLOAD_TOO_LARGE)?;
    let mut headers = BTreeMap::new();
    for name in ["content-type", "accept", "user-agent"] {
        if let Some(value) = parts.headers.get(name).and_then(|v| v.to_str().ok()) {
            if value.len() > 8192 {
                return Err(StatusCode::BAD_REQUEST);
            }
            headers.insert(name, value);
        }
    }
    let query: Vec<(String, String)> =
        url::form_urlencoded::parse(parts.uri.query().unwrap_or("").as_bytes())
            .into_owned()
            .collect();
    let input = serde_json::json!({"method":parts.method.as_str(),"path":parts.uri.path(),"query":query,"headers":headers,"body_base64":STANDARD.encode(bytes)});
    let code = handler_code(&def, &input).map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let (heap_ceiling, timeout_ceiling) = app.engine.http_functions_limits();
    let timeout = def.limits.timeout_seconds.min(timeout_ceiling);
    if heap_ceiling < 8 || timeout == 0 {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    // Keep admission capacity through completion even if the caller disconnects.
    tokio::spawn(
        async move {
            let _permit = permit;
            let id = app
                .engine
                .submit_execution(ExecutionRequest {
                    code,
                    file: None,
                    heap: None,
                    fs: None,
                    session: None,
                    heap_memory_max_mb: Some(def.limits.heap_mb.min(heap_ceiling)),
                    execution_timeout_secs: Some(timeout),
                    tags: None,
                    mcp_headers: None,
                })
                .await
                .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
            tracing::debug!(execution_id = %id, "HTTP function execution submitted");
            // Never resubmit, including after ambiguous submission or client disconnect.
            let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout + 1);
            loop {
                let info = app
                    .engine
                    .get_execution(id.clone())
                    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
                match info.status.as_str() {
                    "completed" => {
                        return response(
                            info.result
                                .as_deref()
                                .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?,
                            &parts.method,
                        );
                    }
                    "timed_out" => return Err(StatusCode::GATEWAY_TIMEOUT),
                    "failed" | "cancelled" => return Err(StatusCode::INTERNAL_SERVER_ERROR),
                    _ => {}
                }
                if tokio::time::Instant::now() >= deadline {
                    let _ = app.engine.cancel_execution(id.clone());
                    return Err(StatusCode::GATEWAY_TIMEOUT);
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
        .in_current_span(),
    )
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
}

async fn request_diagnostics(request: Request, next: axum::middleware::Next) -> Response {
    let id = uuid::Uuid::new_v4().to_string();
    let span = tracing::info_span!("http_function", request_id = %id);
    let mut response = next.run(request).instrument(span.clone()).await;
    response
        .headers_mut()
        .insert("x-request-id", HeaderValue::from_str(&id).unwrap());
    if response.status().is_server_error() {
        let _entered = span.enter();
        tracing::warn!(
            status = response.status().as_u16(),
            "HTTP function request failed"
        );
    }
    response
}

/// Explicitly opt in using MCP_V8_HTTP_FUNCTIONS_DIR. Cluster nodes use the
/// existing replicated data store; the directory is only used in single-node mode.
pub fn configured_router(engine: Arc<Engine>) -> Result<Router, String> {
    let Some(dir) = std::env::var_os("MCP_V8_HTTP_FUNCTIONS_DIR") else {
        return Ok(Router::new());
    };
    if crate::scheduled_worker::configured() {
        return Err("HTTP functions forbidden on scheduled-only workers".into());
    }
    let backend = match engine.http_functions_cluster() {
        Some(node) => Backend::Cluster(node),
        None => Backend::Local(
            sled::open(dir)
                .and_then(|db| db.open_tree("http-functions-v1"))
                .map_err(|e| e.to_string())?,
        ),
    };
    Ok(router(
        engine,
        Arc::new(Registry {
            backend,
            lock: Mutex::new(()),
        }),
    ))
}
fn router(engine: Arc<Engine>, registry: Arc<Registry>) -> Router {
    Router::new()
        .route("/api/http-functions/v1/definitions", get(list))
        .route(
            "/api/http-functions/v1/definitions/{name}",
            get(lookup).put(put).delete(delete),
        )
        .route("/functions/v1/{name}", axum::routing::any(invoke))
        .layer(axum::middleware::from_fn(request_diagnostics))
        .with_state(App {
            engine,
            registry,
            slots: Arc::new(Semaphore::new(16)),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::ffi_config::{EngineConfigBuilder, ExecutionLimitsBuilder};
    use tower::ServiceExt;
    fn fixture(dir: &std::path::Path) -> (Arc<Engine>, Arc<Registry>, Router) {
        let engine = Engine::create(
            EngineConfigBuilder::new()
                .data_dir(dir.join("engine").to_str().unwrap().to_owned())
                .limits(
                    ExecutionLimitsBuilder::new()
                        .heap_memory_max_mb(64)
                        .execution_timeout_secs(3)
                        .build()
                        .unwrap(),
                )
                .build()
                .unwrap(),
        )
        .unwrap();
        let registry = Arc::new(Registry {
            backend: Backend::Local(
                sled::open(dir.join("registry"))
                    .unwrap()
                    .open_tree("functions")
                    .unwrap(),
            ),
            lock: Mutex::new(()),
        });
        let app = router(engine.clone(), registry.clone());
        (engine, registry, app)
    }
    async fn send(
        app: &Router,
        method: &str,
        uri: &str,
        condition: Option<(&str, &str)>,
        body: String,
    ) -> Response {
        let mut req = Request::builder().method(method).uri(uri);
        if let Some((key, value)) = condition {
            req = req.header(key, value);
        }
        app.clone()
            .oneshot(req.body(Body::from(body)).unwrap())
            .await
            .unwrap()
    }
    fn definition(source: &str) -> String {
        serde_json::json!({"version":1,"methods":["POST","HEAD"],"source":source,"export":"handler","limits":{"timeout_seconds":1,"heap_mb":32}}).to_string()
    }
    #[tokio::test]
    async fn cluster_barrier_failover_and_quorum_loss() {
        use crate::cluster::{AppendEntriesRequest, ClusterConfig, Role};
        let db = sled::Config::new().temporary(true).open().unwrap();
        let leader = ClusterNode::new(ClusterConfig::default(), db.clone());
        leader.state.write().await.current_term = 1;
        leader.sim_install_elected_leader(1).await;
        let pump_node = leader.clone();
        let pump = tokio::spawn(async move {
            loop {
                pump_node.sim_commit().await;
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        });
        let registry = Registry {
            backend: Backend::Cluster(leader.clone()),
            lock: Mutex::new(()),
        };
        let mut headers = HeaderMap::new();
        headers.insert("if-none-match", HeaderValue::from_static("*"));
        let def = serde_json::from_str(&definition(
            "function handler(){return {status:200,body:'ok'}}",
        ))
        .unwrap();
        let original = registry
            .change("echo".into(), &headers, Some(def))
            .await
            .unwrap();
        let follower = ClusterNode::new(
            ClusterConfig::default(),
            sled::Config::new().temporary(true).open().unwrap(),
        );
        let entries = leader.state.read().await.log.clone();
        let commit = leader.state.read().await.commit_index;
        assert!(
            follower
                .handle_append_entries(AppendEntriesRequest {
                    term: 1,
                    leader_id: "leader".into(),
                    prev_log_index: 0,
                    prev_log_term: 0,
                    entries,
                    leader_commit: commit,
                    peer_addrs: None,
                    learner_addrs: None,
                })
                .await
                .success
        );
        let next = Registry {
            backend: Backend::Cluster(follower.clone()),
            lock: Mutex::new(()),
        };
        assert_eq!(
            next.read().await.err(),
            Some(StatusCode::SERVICE_UNAVAILABLE)
        );
        follower.state.write().await.current_term = 2;
        follower.sim_install_elected_leader(2).await;
        let pump_node = follower.clone();
        let next_pump = tokio::spawn(async move {
            loop {
                pump_node.sim_commit().await;
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        });
        assert_eq!(
            next.read().await.unwrap()["echo"].revision,
            original.revision
        );
        headers.clear();
        headers.insert(
            "if-match",
            HeaderValue::from_str(&format!("\"{}\"", original.revision)).unwrap(),
        );
        let deleted = next.change("echo".into(), &headers, None).await.unwrap();
        assert!(next.read().await.unwrap()["echo"].definition.is_none());
        // A retained stale cache cannot admit after losing leadership.
        leader.state.write().await.role = Role::Follower;
        assert_eq!(
            registry.read().await.err(),
            Some(StatusCode::SERVICE_UNAVAILABLE)
        );
        pump.abort();
        let committed = follower.get(KEY).await.unwrap();
        follower.state.write().await.peers = vec!["unreachable-a".into(), "unreachable-b".into()];
        assert!(
            tokio::time::timeout(Duration::from_millis(100), next.read())
                .await
                .is_err()
        );
        assert_eq!(follower.get(KEY).await.unwrap(), committed);
        next_pump.abort();
        // Leave a different registry value in the prior-term uncommitted tail.
        // A CAS-only read cannot advance this suffix after the next election.
        let mut pending: Records = serde_json::from_str(committed.as_ref().unwrap()).unwrap();
        pending.insert(
            "pending".into(),
            Entry {
                revision: uuid::Uuid::new_v4().to_string(),
                definition: Some(
                    serde_json::from_str(&definition(
                        "function handler(){return {status:200,body:'pending'}}",
                    ))
                    .unwrap(),
                ),
            },
        );
        assert!(
            tokio::time::timeout(
                Duration::from_millis(100),
                follower.cas(
                    KEY.into(),
                    committed.clone(),
                    serde_json::to_string(&pending).unwrap()
                )
            )
            .await
            .is_err()
        );
        assert_eq!(follower.get(KEY).await.unwrap(), committed);
        let restarted = ClusterNode::new(ClusterConfig::default(), follower.db.clone());
        assert_eq!(restarted.get(KEY).await.unwrap(), committed);
        let restored = Registry {
            backend: Backend::Cluster(restarted.clone()),
            lock: Mutex::new(()),
        };
        assert_eq!(
            restored.read().await.err(),
            Some(StatusCode::SERVICE_UNAVAILABLE)
        );
        restarted.state.write().await.current_term = 3;
        restarted.sim_install_elected_leader(3).await;
        let pump_node = restarted.clone();
        let restored_pump = tokio::spawn(async move {
            loop {
                pump_node.sim_commit().await;
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        });
        let recovered = tokio::time::timeout(Duration::from_secs(2), restored.read())
            .await
            .unwrap()
            .unwrap();
        assert!(recovered["echo"].definition.is_none());
        assert!(recovered["pending"].definition.is_some());
        restored_pump.abort();
        assert_eq!(
            serde_json::from_str::<Records>(&committed.unwrap()).unwrap()["echo"].revision,
            deleted.revision
        );
    }

    #[tokio::test]
    async fn module_imports_and_corrupt_registry_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        let (engine, registry, app) = fixture(dir.path());
        let source = "import { basename } from 'node:path'; async function handler() { return {status:200,body:basename('/a/module.txt')}; }";
        assert_eq!(
            send(
                &app,
                "PUT",
                "/api/http-functions/v1/definitions/module",
                Some(("if-none-match", "*")),
                definition(source)
            )
            .await
            .status(),
            StatusCode::OK
        );
        let result = send(&app, "POST", "/functions/v1/module", None, String::new()).await;
        assert_eq!(result.status(), StatusCode::OK);
        assert_eq!(
            to_bytes(result.into_body(), MAX_BODY)
                .await
                .unwrap()
                .as_ref(),
            b"module.txt"
        );
        if let Backend::Local(tree) = &registry.backend {
            tree.insert(KEY, b"{invalid registry").unwrap();
        }
        assert_eq!(
            send(&app, "POST", "/functions/v1/module", None, String::new())
                .await
                .status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(engine.list_executions().unwrap().len(), 1);
        engine.shutdown().await;
    }

    #[tokio::test]
    async fn replacement_deletion_and_disconnect_do_not_replay_admitted_work() {
        let dir = tempfile::tempdir().unwrap();
        let (engine, _, app) = fixture(dir.path());
        let uri = "/api/http-functions/v1/definitions/pinned";
        let old = definition(
            "async function handler() { await new Promise(r=>setTimeout(r,250)); return {status:200,body:'old'}; }",
        );
        let created = send(&app, "PUT", uri, Some(("if-none-match", "*")), old).await;
        let etag = created.headers()["etag"].to_str().unwrap().to_owned();
        let request_app = app.clone();
        let pending = tokio::spawn(async move {
            send(
                &request_app,
                "POST",
                "/functions/v1/pinned",
                None,
                String::new(),
            )
            .await
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            while engine.list_executions().unwrap().is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let updated = send(
            &app,
            "PUT",
            uri,
            Some(("if-match", &etag)),
            definition("function handler(){ return {status:200,body:'new'}; }"),
        )
        .await;
        assert_eq!(updated.status(), StatusCode::OK);
        let etag = updated.headers()["etag"].to_str().unwrap().to_owned();
        assert_eq!(
            send(
                &app,
                "DELETE",
                uri,
                Some(("if-match", &etag)),
                String::new()
            )
            .await
            .status(),
            StatusCode::OK
        );
        assert_eq!(
            send(&app, "POST", "/functions/v1/pinned", None, String::new())
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            to_bytes(pending.await.unwrap().into_body(), 100)
                .await
                .unwrap()
                .as_ref(),
            b"old"
        );
        assert_eq!(engine.list_executions().unwrap().len(), 1);
        send(&app, "PUT", uri, Some(("if-none-match", "*")), definition("async function handler(){ await new Promise(r=>setTimeout(r,100)); return {status:200,body:'done'}; }")).await;
        let request_app = app.clone();
        let disconnected = tokio::spawn(async move {
            send(
                &request_app,
                "POST",
                "/functions/v1/pinned",
                None,
                String::new(),
            )
            .await
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            while engine.list_executions().unwrap().len() != 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        disconnected.abort();
        tokio::time::timeout(Duration::from_secs(2), async {
            while engine
                .list_executions()
                .unwrap()
                .iter()
                .any(|e| e.status == "running")
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(engine.list_executions().unwrap().len(), 2);
        engine.shutdown().await;
    }

    #[tokio::test]
    async fn validation_response_and_body_limits() {
        for (value, method, expected) in [
            (
                serde_json::json!({"status":200,"body":"hello"}),
                Method::HEAD,
                200,
            ),
            (
                serde_json::json!({"status":205,"body":"hello"}),
                Method::GET,
                205,
            ),
            (
                serde_json::json!({"status":304,"body":"hello"}),
                Method::GET,
                304,
            ),
        ] {
            let reply = response(&value.to_string(), &method).unwrap();
            assert_eq!(reply.status().as_u16(), expected);
            assert!(
                to_bytes(reply.into_body(), MAX_BODY)
                    .await
                    .unwrap()
                    .is_empty()
            );
        }
        assert!(
            response(
                &serde_json::json!({"status":200,"body":"x".repeat(MAX_BODY+1)}).to_string(),
                &Method::GET
            )
            .is_err()
        );
        let dir = tempfile::tempdir().unwrap();
        let (engine, _, app) = fixture(dir.path());
        for source in [
            "function handler( {",
            "throw new Error('registration executed')",
        ] {
            let result = send(
                &app,
                "PUT",
                "/api/http-functions/v1/definitions/check",
                Some(("if-none-match", "*")),
                definition(source),
            )
            .await;
            assert_eq!(
                result.status().as_u16(),
                if source.starts_with("throw") {
                    200
                } else {
                    400
                }
            );
        }
        assert_eq!(
            send(
                &app,
                "POST",
                "/functions/v1/check",
                None,
                "x".repeat(MAX_BODY + 1)
            )
            .await
            .status(),
            StatusCode::PAYLOAD_TOO_LARGE
        );
        engine.shutdown().await;
    }

    #[tokio::test]
    async fn table_driven_http_lifecycle_and_real_engine() {
        let dir = tempfile::tempdir().unwrap();
        let (engine, _, app) = fixture(dir.path());
        let echo = definition(
            "async function handler(request) { await Promise.resolve(); console.log('not the response'); return {status:200, body:JSON.stringify(request)}; }",
        );
        let uri = "/api/http-functions/v1/definitions/echo";
        for (label, method, path, condition, body, expected) in [
            (
                "missing",
                "POST",
                "/functions/v1/echo",
                None,
                String::new(),
                404,
            ),
            ("precondition required", "PUT", uri, None, echo.clone(), 428),
            (
                "bad name",
                "PUT",
                "/api/http-functions/v1/definitions/api",
                Some(("if-none-match", "*")),
                echo.clone(),
                400,
            ),
            (
                "create",
                "PUT",
                uri,
                Some(("if-none-match", "*")),
                echo.clone(),
                200,
            ),
            (
                "duplicate",
                "PUT",
                uri,
                Some(("if-none-match", "*")),
                echo.clone(),
                412,
            ),
            (
                "method",
                "GET",
                "/functions/v1/echo",
                None,
                String::new(),
                405,
            ),
            (
                "stale",
                "PUT",
                uri,
                Some(("if-match", "\"stale\"")),
                echo.clone(),
                412,
            ),
            ("metadata", "GET", uri, None, String::new(), 200),
            (
                "list",
                "GET",
                "/api/http-functions/v1/definitions",
                None,
                String::new(),
                200,
            ),
        ] {
            assert_eq!(
                send(&app, method, path, condition, body)
                    .await
                    .status()
                    .as_u16(),
                expected,
                "{label}"
            );
        }
        let payload = "\"}); throw new Error('injected'); //\0binary";
        let result = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/functions/v1/echo?a=1&a=2")
                    .header("authorization", "Bearer secret")
                    .header("cookie", "secret")
                    .body(Body::from(payload))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(result.status(), StatusCode::OK);
        let body = to_bytes(result.into_body(), MAX_BODY).await.unwrap();
        let input: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(input["body_base64"], STANDARD.encode(payload));
        assert_eq!(input["query"], serde_json::json!([["a", "1"], ["a", "2"]]));
        assert!(input["headers"].get("authorization").is_none());
        assert!(input["headers"].get("cookie").is_none());
        let revision = send(&app, "GET", uri, None, String::new()).await.headers()["etag"]
            .to_str()
            .unwrap()
            .to_owned();
        let fresh = definition(
            "async function handler() { globalThis.counter=(globalThis.counter||0)+1; return {status:200, body:String(globalThis.counter)}; }",
        );
        assert_eq!(
            send(&app, "PUT", uri, Some(("if-match", &revision)), fresh)
                .await
                .status(),
            StatusCode::OK
        );
        for _ in 0..2 {
            let reply = send(&app, "POST", "/functions/v1/echo", None, String::new()).await;
            assert_eq!(
                to_bytes(reply.into_body(), 100).await.unwrap().as_ref(),
                b"1"
            );
        }
        let revision = send(&app, "GET", uri, None, String::new()).await.headers()["etag"]
            .to_str()
            .unwrap()
            .to_owned();
        assert_eq!(
            send(
                &app,
                "DELETE",
                uri,
                Some(("if-match", &revision)),
                String::new()
            )
            .await
            .status(),
            StatusCode::OK
        );
        assert_eq!(
            send(&app, "POST", "/functions/v1/echo", None, String::new())
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
        engine.shutdown().await;
    }
    #[tokio::test]
    async fn table_driven_handler_results() {
        let dir = tempfile::tempdir().unwrap();
        let (engine, _, app) = fixture(dir.path());
        for (index, (source, status, body)) in [
            (
                "return {status:201, body:'ok'}",
                201,
                Some(b"ok".as_slice()),
            ),
            (
                "return {status:200, body_base64:'AAH/'}",
                200,
                Some(b"\0\x01\xff".as_slice()),
            ),
            (
                "return {status:204, body:'suppressed'}",
                204,
                Some(b"".as_slice()),
            ),
            ("return {status:200, body_base64:'!'}", 500, None),
            ("return {status:99, body:''}", 500, None),
            ("return {status:200, body:'', body_base64:''}", 500, None),
            (
                "return {status:200, headers:{connection:'close'}, body:''}",
                500,
                None,
            ),
            (
                "return {status:200, headers:{x:'bad\\r\\nheader'}, body:''}",
                500,
                None,
            ),
            ("throw new Error('private stack')", 500, None),
            ("while (true) {}", 504, None),
        ]
        .iter()
        .enumerate()
        {
            let name = format!("case{index}");
            let definition = definition(&format!("async function handler() {{ {source} }}"));
            assert_eq!(
                send(
                    &app,
                    "PUT",
                    &format!("/api/http-functions/v1/definitions/{name}"),
                    Some(("if-none-match", "*")),
                    definition
                )
                .await
                .status(),
                StatusCode::OK
            );
            let result = send(
                &app,
                "POST",
                &format!("/functions/v1/{name}"),
                None,
                String::new(),
            )
            .await;
            assert_eq!(result.status().as_u16(), *status, "case {index}");
            let actual = to_bytes(result.into_body(), MAX_BODY * 2).await.unwrap();
            if let Some(body) = body {
                assert_eq!(actual.as_ref(), *body, "case {index}");
            }
            assert!(!String::from_utf8_lossy(&actual).contains("private stack"));
        }
        engine.shutdown().await;
    }
    #[tokio::test]
    async fn registry_restart_tombstone_and_concurrent_cas() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("registry");
        let open = || Registry {
            backend: Backend::Local(
                sled::open(&db_path)
                    .unwrap()
                    .open_tree("functions")
                    .unwrap(),
            ),
            lock: Mutex::new(()),
        };
        let mut headers = HeaderMap::new();
        headers.insert("if-none-match", HeaderValue::from_static("*"));
        let def: Definition = serde_json::from_str(&definition(
            "function handler() { return {status:200,body:''}; }",
        ))
        .unwrap();
        let registry = open();
        let first = registry
            .change("x".into(), &headers, Some(def.clone()))
            .await
            .unwrap();
        headers.clear();
        headers.insert(
            "if-match",
            HeaderValue::from_str(&format!("\"{}\"", first.revision)).unwrap(),
        );
        let (a, b) = tokio::join!(
            registry.change("x".into(), &headers, Some(def.clone())),
            registry.change("x".into(), &headers, Some(def))
        );
        assert_ne!(a.is_ok(), b.is_ok());
        let current = registry.read().await.unwrap().remove("x").unwrap();
        headers.insert(
            "if-match",
            HeaderValue::from_str(&format!("\"{}\"", current.revision)).unwrap(),
        );
        let deleted = registry.change("x".into(), &headers, None).await.unwrap();
        drop(registry);
        let registry = open();
        let persisted = registry.read().await.unwrap().remove("x").unwrap();
        assert_eq!(persisted.revision, deleted.revision);
        assert!(persisted.definition.is_none());
        assert_eq!(
            registry.change("x".into(), &headers, None).await.err(),
            Some(StatusCode::PRECONDITION_FAILED)
        );
    }
}
