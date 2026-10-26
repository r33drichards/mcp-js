//! Opt-in operator transports. All commands are serialized by the runner loop.
//! Neither a transport timeout nor a dropped response cancels an accepted command.
use crate::{
    Replicated,
    cluster::{ClusterNode, Role},
    scheduled_worker::secret,
};
use axum::{
    Json, Router,
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::Utc;
use mcp_js_scheduler_core::Definition;
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ListToolsResult,
        PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
    },
    service::RequestContext,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub addr: std::net::SocketAddr,
    pub bearer_env: String,
    /// Optional, separate credential for human-attested recovery.
    pub recovery_bearer_env: Option<String>,
}
#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    Status,
    List,
    Create {
        definition: Definition,
    },
    Disable {
        id: u64,
    },
    Cancel {
        occurrence_id: u64,
    },
    Delete {
        id: u64,
    },
    AcknowledgeStopped {
        occurrence_id: u64,
        dispatcher_and_work_quiesced: bool,
    },
}
impl Command {
    fn recovery(&self) -> bool {
        matches!(self, Self::AcknowledgeStopped { .. })
    }
}

fn parse_command(value: Value) -> Result<Command, ()> {
    fn keys(value: &Value, allowed: &[&str]) -> Result<(), ()> {
        let object = value.as_object().ok_or(())?;
        if object.keys().any(|k| !allowed.contains(&k.as_str())) {
            return Err(());
        }
        Ok(())
    }
    let action = value.get("action").and_then(Value::as_str).ok_or(())?;
    let allowed: &[&str] = match action {
        "status" | "list" => &["action"],
        "create" => &["action", "definition"],
        "disable" | "delete" => &["action", "id"],
        "cancel" => &["action", "occurrence_id"],
        "acknowledge_stopped" => &["action", "occurrence_id", "dispatcher_and_work_quiesced"],
        _ => return Err(()),
    };
    keys(&value, allowed)?;
    if action == "create" {
        let definition = &value["definition"];
        keys(definition, &["name", "code", "cron", "timezone", "policy"])?;
        keys(&definition["policy"], &["missed", "overlap", "retry"])?;
        keys(
            &definition["policy"]["retry"],
            &["max_retries", "delay_seconds"],
        )?;
        if let Some(catch_up) = definition["policy"]["missed"].get("CatchUp") {
            keys(&definition["policy"]["missed"], &["CatchUp"])?;
            keys(catch_up, &["cap"])?;
        }
    }
    serde_json::from_value(value).map_err(|_| ())
}

/// Shared with stdin; remote adapters replace internal errors with safe categories.
pub async fn execute(
    command: Command,
    node: &Arc<ClusterNode>,
    owner: &mut Option<Replicated>,
) -> Result<Value, String> {
    if matches!(command, Command::Status) {
        let status = node.status().await;
        let acquired = status.role == Role::Leader
            && owner
                .as_ref()
                .is_some_and(|a| a.is_valid() && a.term() == status.term);
        return Ok(json!({"cluster": status, "owner_acquired": acquired}));
    }
    let active = owner
        .as_mut()
        .ok_or("no acquired leader; send command to current leader")?;
    match command {
        Command::Status => unreachable!(),
        Command::List => Ok(
            json!({"schedules": active.view().schedules(), "occurrences": active.view().occurrences(), "stale_local_view": true}),
        ),
        Command::Create { definition } => active
            .create(definition, Utc::now())
            .await
            .map(|id| json!({"id":id})),
        Command::Disable { id } => active.disable(id).await.map(|_| json!({"ok":true})),
        Command::Cancel { occurrence_id } => active
            .request_cancel(occurrence_id)
            .await
            .map(|_| json!({"ok":true,"cancellation_requested":true})),
        Command::Delete { id } => active.delete(id).await.map(|_| json!({"ok":true})),
        Command::AcknowledgeStopped {
            occurrence_id,
            dispatcher_and_work_quiesced: true,
        } => active
            .acknowledge_stopped(occurrence_id)
            .await
            .map(|_| json!({"ok":true})),
        Command::AcknowledgeStopped { .. } => {
            Err("must establish old dispatcher cannot submit and in-flight work is resolved".into())
        }
    }
}

pub type Call = (Command, oneshot::Sender<Result<Value, String>>);
#[derive(Clone)]
pub struct Transport {
    operator: Arc<str>,
    recovery: Option<Arc<str>>,
    tx: mpsc::Sender<Call>,
}
// Hash to fixed-size values before comparison; credential bytes never appear in errors.
fn same(a: &str, b: &str) -> bool {
    use sha2::{Digest, Sha256};
    use subtle::ConstantTimeEq;
    Sha256::digest(a.as_bytes())
        .ct_eq(&Sha256::digest(b.as_bytes()))
        .into()
}
impl Transport {
    pub fn new(
        config: &Config,
        admission: &str,
        worker: &str,
        cluster: &str,
        tx: mpsc::Sender<Call>,
    ) -> Result<Self, String> {
        let operator = secret(&config.bearer_env)?;
        let recovery = config
            .recovery_bearer_env
            .as_deref()
            .map(secret)
            .transpose()?;
        if same(&operator, admission)
            || same(&operator, worker)
            || same(&operator, cluster)
            || recovery.as_deref().is_some_and(|r| {
                same(r, &operator) || same(r, admission) || same(r, worker) || same(r, cluster)
            })
        {
            return Err("management credentials must be separate from each other and worker/admission/cluster credentials".into());
        }
        // Keep operator authority on loopback even with authenticated Raft.
        // Authenticated TLS proxies may connect locally.
        if !config.addr.ip().is_loopback() {
            return Err("management listener must bind a loopback address".into());
        }
        Ok(Self {
            operator: operator.into(),
            recovery: recovery.map(Into::into),
            tx,
        })
    }
    fn authorized(&self, headers: &HeaderMap, recovery: bool) -> bool {
        let Some(token) = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
        else {
            return false;
        };
        self.recovery.as_deref().is_some_and(|r| same(r, token))
            || (!recovery && same(&self.operator, token))
    }
    async fn call(
        &self,
        command: Command,
        headers: &HeaderMap,
    ) -> Result<Value, (StatusCode, Value)> {
        if !self.authorized(headers, command.recovery()) {
            return Err((
                StatusCode::FORBIDDEN,
                json!({"error":"recovery_authorization_required"}),
            ));
        }
        if matches!(
            command,
            Command::AcknowledgeStopped {
                dispatcher_and_work_quiesced: false,
                ..
            }
        ) {
            return Err((
                StatusCode::BAD_REQUEST,
                json!({"error":"explicit_quiescence_attestation_required"}),
            ));
        }
        let (reply, rx) = oneshot::channel();
        // Bounded admission; reject before enqueue instead of retaining unbounded requests.
        self.tx.try_send((command, reply)).map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                json!({"error":"management_unavailable","accepted":false}),
            )
        })?;
        match rx.await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(e)) if e == "no acquired leader; send command to current leader" => Err((
                StatusCode::CONFLICT,
                json!({"error":"no_acquired_owner","accepted":false}),
            )),
            // The core returns strings for both validation and uncertain storage errors.
            // Do not leak source/store details or claim a failed command did not commit.
            _ => Err((
                StatusCode::SERVICE_UNAVAILABLE,
                json!({"error":"operation_failed","outcome":"uncertain","retry_safe":false}),
            )),
        }
    }
    pub fn router(self) -> Router {
        let factory = self.clone();
        let mut config = StreamableHttpServerConfig::default().enforce_origin_validation();
        config.legacy_session_mode = false;
        config.json_response = true;
        config.max_request_body_bytes = 72 * 1024;
        let mcp = StreamableHttpService::new(
            move || Ok(factory.clone()),
            LocalSessionManager::default().into(),
            config,
        );
        Router::new()
            .route("/scheduler/v1/status", get(status))
            .route("/scheduler/v1/schedules", get(list))
            .route("/scheduler/v1/commands", post(command))
            .nest_service("/mcp", mcp)
            .layer(axum::extract::DefaultBodyLimit::max(72 * 1024))
            .layer(axum::middleware::from_fn_with_state(
                self.clone(),
                authenticate,
            ))
            .with_state(self)
    }
}
async fn authenticate(State(state): State<Transport>, request: Request, next: Next) -> Response {
    if !state.authorized(request.headers(), false) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"unauthorized"})),
        )
            .into_response();
    }
    // Management has no browser flow. Reject all cross-origin/browser submissions.
    if request.headers().contains_key("origin") {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error":"origin_not_allowed"})),
        )
            .into_response();
    }
    next.run(request).await
}
fn response(result: Result<Value, (StatusCode, Value)>) -> Response {
    match result {
        Ok(value) => Json(value).into_response(),
        Err((status, value)) => (status, Json(value)).into_response(),
    }
}
async fn status(State(state): State<Transport>, headers: HeaderMap) -> Response {
    response(state.call(Command::Status, &headers).await)
}
async fn list(State(state): State<Transport>, headers: HeaderMap) -> Response {
    response(state.call(Command::List, &headers).await)
}
async fn command(
    State(state): State<Transport>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let Ok(command) = serde_json::from_slice::<Value>(&body)
        .map_err(|_| ())
        .and_then(parse_command)
    else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":"invalid_command"})),
        )
            .into_response();
    };
    response(state.call(command, &headers).await)
}
fn definition_schema() -> Value {
    json!({"type":"object","additionalProperties":false,"required":["code","cron","timezone","policy"],"properties":{
        "name":{"type":["string","null"],"maxLength":128},
        "code":{"type":"string","maxLength":65536},
        "cron":{"type":"string","maxLength":256,"description":"Numeric five-field cron"},
        "timezone":{"type":"string","maxLength":128,"description":"IANA timezone"},
        "policy":{"type":"object","additionalProperties":false,"required":["missed","overlap","retry"],"properties":{
            "missed":{"oneOf":[{"enum":["Skip","RunOnce"]},{"type":"object","additionalProperties":false,"required":["CatchUp"],"properties":{"CatchUp":{"type":"object","additionalProperties":false,"required":["cap"],"properties":{"cap":{"type":"integer","minimum":1}}}}}]},
            "overlap":{"enum":["Skip","Queue","Concurrent"]},
            "retry":{"type":"object","additionalProperties":false,"required":["max_retries","delay_seconds"],"properties":{"max_retries":{"type":"integer","minimum":0,"maximum":100},"delay_seconds":{"type":"integer","minimum":0,"maximum":86400}}}
        }}
    }})
}

fn tools() -> Vec<Tool> {
    let mut result = Vec::new();
    for (name, description, properties, required) in [
        (
            "status",
            "Inspect runner ownership; election alone does not confer ownership.",
            json!({}),
            json!([]),
        ),
        (
            "list",
            "Inspect schedules and occurrences. Explicitly a stale local view.",
            json!({}),
            json!([]),
        ),
        (
            "create",
            "Create a schedule using existing core policies. Never retry an uncertain result automatically.",
            json!({"definition": definition_schema()}),
            json!(["definition"]),
        ),
        (
            "disable",
            "Disable future occurrences without cancelling accepted work.",
            json!({"id":{"type":"integer","minimum":1}}),
            json!(["id"]),
        ),
        (
            "cancel",
            "Request cancellation of one occurrence, independently of schedule enablement. Capacity remains reserved until confirmed stopped.",
            json!({"occurrence_id":{"type":"integer","minimum":1}}),
            json!(["occurrence_id"]),
        ),
        (
            "delete",
            "Delete a schedule using existing core semantics; this does not establish stopped external effects.",
            json!({"id":{"type":"integer","minimum":1}}),
            json!(["id"]),
        ),
        (
            "acknowledge_stopped",
            "Privileged operator recovery. Requires separate recovery authorization and explicit human evidence that the old dispatcher cannot submit and accepted work is quiesced. Never infer or auto-approve this attestation. Does not cancel a permit or undo external effects.",
            json!({"occurrence_id":{"type":"integer","minimum":1},"dispatcher_and_work_quiesced":{"type":"boolean","const":true}}),
            json!(["occurrence_id", "dispatcher_and_work_quiesced"]),
        ),
    ] {
        let schema = json!({"type":"object","properties":properties,"required":required,"additionalProperties":false});
        result.push(Tool::new(
            format!("scheduler_{name}"),
            description,
            Arc::new(schema.as_object().unwrap().clone()),
        ));
    }
    result
}
impl ServerHandler for Transport {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build()).with_instructions("Operator-owned scheduler management. Uncertain mutations must not be automatically retried. Recovery requires a human quiescence attestation and a separate credential.")
    }
    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let recovery = context
            .extensions
            .get::<axum::http::request::Parts>()
            .is_some_and(|p| self.authorized(&p.headers, true));
        Ok(ListToolsResult::with_all_items(
            tools()
                .into_iter()
                .filter(|t| recovery || t.name != "scheduler_acknowledge_stopped")
                .collect(),
        ))
    }
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let name = request
            .name
            .strip_prefix("scheduler_")
            .ok_or_else(|| McpError::invalid_params("unknown tool", None))?;
        if !tools().iter().any(|t| t.name == request.name) {
            return Err(McpError::invalid_params("unknown tool", None));
        }
        let mut args = request.arguments.unwrap_or_default();
        if args.contains_key("action") {
            return Err(McpError::invalid_params("invalid arguments", None));
        }
        args.insert("action".into(), json!(name));
        let command = parse_command(Value::Object(args))
            .map_err(|_| McpError::invalid_params("invalid arguments", None))?;
        let headers = context
            .extensions
            .get::<axum::http::request::Parts>()
            .map(|p| p.headers.clone())
            .unwrap_or_default();
        let result = match self.call(command, &headers).await {
            Ok(value) => CallToolResult::success(vec![ContentBlock::text(value.to_string())]),
            Err((status, mut value)) => {
                value["status"] = json!(status.as_u16());
                CallToolResult::error(vec![ContentBlock::text(value.to_string())])
            }
        };
        Ok(result.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_are_separate_and_management_is_loopback_only() {
        let value = std::env::var("PATH").unwrap();
        let (tx, _) = mpsc::channel(1);
        let mut config = Config {
            addr: "127.0.0.1:0".parse().unwrap(),
            bearer_env: "PATH".into(),
            recovery_bearer_env: None,
        };
        assert!(Transport::new(&config, &value, "different", "cluster", tx.clone()).is_err());
        assert!(Transport::new(&config, "different", &value, "cluster", tx.clone()).is_err());
        assert!(Transport::new(&config, "different", "another", &value, tx.clone()).is_err());
        config.recovery_bearer_env = Some("PATH".into());
        assert!(Transport::new(&config, "different", "another", "cluster", tx.clone()).is_err());
        config.bearer_env = "HOME".into();
        assert!(Transport::new(&config, "different", "another", &value, tx.clone()).is_err());
        config.recovery_bearer_env = None;
        config.addr = "0.0.0.0:0".parse().unwrap();
        assert!(Transport::new(&config, "different", "another", "cluster", tx).is_err());
    }

    #[tokio::test]
    async fn bounded_queue_and_lost_reply_have_distinct_safe_errors() {
        let (tx, mut rx) = mpsc::channel(1);
        let state = Transport {
            operator: "operator".into(),
            recovery: None,
            tx,
        };
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Bearer operator".parse().unwrap());
        let (reply, _) = oneshot::channel();
        state.tx.try_send((Command::Status, reply)).unwrap();
        let full = state.call(Command::Status, &headers).await.unwrap_err();
        assert_eq!(full.0, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(full.1["accepted"], false);
        rx.recv().await.unwrap();
        let call = state.call(Command::Status, &headers);
        let lost_reply = async {
            let (_, reply) = rx.recv().await.unwrap();
            drop(reply);
        };
        let (result, _) = tokio::join!(call, lost_reply);
        assert_eq!(
            result.unwrap_err().1,
            json!({"error":"operation_failed","outcome":"uncertain","retry_safe":false})
        );
        let call = state.call(Command::Status, &headers);
        let no_owner = async {
            let (_, reply) = rx.recv().await.unwrap();
            let _ = reply.send(Err(
                "no acquired leader; send command to current leader".into()
            ));
        };
        let (result, _) = tokio::join!(call, no_owner);
        assert_eq!(
            result.unwrap_err(),
            (
                StatusCode::CONFLICT,
                json!({"error":"no_acquired_owner","accepted":false})
            )
        );
        assert_eq!(
            state
                .call(
                    Command::AcknowledgeStopped {
                        occurrence_id: 1,
                        dispatcher_and_work_quiesced: true
                    },
                    &headers
                )
                .await
                .unwrap_err()
                .0,
            StatusCode::FORBIDDEN
        );
        assert!(rx.try_recv().is_err(), "denied recovery must never enqueue");
    }

    #[test]
    fn remote_schema_rejects_unknown_nested_fields() {
        let definition = json!({"code":"1","cron":"* * * * *","timezone":"UTC","policy":{"missed":"Skip","overlap":"Skip","retry":{"max_retries":0,"delay_seconds":0}}});
        let mut value = json!({"action":"create","definition":definition});
        assert!(parse_command(value.clone()).is_ok());
        value["definition"]["policy"]["retry"]["unexpected"] = json!(true);
        assert!(parse_command(value).is_err());
        assert!(parse_command(json!({"action":"status","unexpected":true})).is_err());
    }
}
