use chrono::Utc;
use mcp_js_scheduler_core::{Definition, Limits, PulseSource};
use mcp_js_scheduler_runner::{
    RemoteConfig, RemoteExecutor, Replicated,
    cluster::{ClusterConfig, ClusterNode, Role},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    task::JoinSet,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    node_id: String,
    advertise_addr: String,
    cluster_port: u16,
    peers: Vec<String>,
    data_dir: String,
    admission_addr: String,
    admission_bearer_env: String,
    #[serde(default)]
    limits: Limits,
    #[serde(default)]
    schedules: Vec<Definition>,
    remote: RemoteConfig,
}
#[derive(Serialize)]
struct Contract<'a> {
    limits: &'a Limits,
    schedules: &'a [Definition],
    remote: &'a RemoteConfig,
}
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Command {
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

#[tokio::main]
async fn main() -> Result<(), String> {
    let path = std::env::args()
        .nth(1)
        .ok_or("usage: mcp-js-scheduler-runner CONFIG.json")?;
    let config: Config = serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let admission_token =
        mcp_js_scheduler_runner::scheduled_worker::secret(&config.admission_bearer_env)?;
    let (admission_tx, mut admission_rx) = tokio::sync::mpsc::channel::<AdmissionCall>(64);
    let admission_listener = tokio::net::TcpListener::bind(&config.admission_addr)
        .await
        .map_err(|e| e.to_string())?;
    let admission_router = axum::Router::new()
        .route(
            "/scheduler/v1/admit",
            axum::routing::post(admission_handler),
        )
        .layer(axum::extract::DefaultBodyLimit::max(16 * 1024 * 1024))
        .with_state((admission_token, admission_tx));
    let admission_server =
        tokio::spawn(async move { axum::serve(admission_listener, admission_router).await });
    let executor = Arc::new(RemoteExecutor::new(config.remote.clone())?);
    // All replicas must use the same grant reference/expiry, limits and initial definitions.
    let contract = serde_json::to_string(&Contract {
        limits: &config.limits,
        schedules: &config.schedules,
        remote: &config.remote,
    })
    .map_err(|e| e.to_string())?;
    let (peers, peer_addrs) = ClusterConfig::parse_peers(&config.peers);
    let path = std::path::Path::new(&config.data_dir);
    let existing = path.exists()
        && std::fs::read_dir(path)
            .map_err(|e| e.to_string())?
            .next()
            .is_some();
    if existing && !path.join("conf").is_file() {
        return Err("existing data directory is not a supported sealed Raft store; explicit migration required".into());
    }
    let db = sled::open(path).map_err(|e| e.to_string())?;
    if existing
        && db
            .get("raft_fixed_voters")
            .map_err(|e| e.to_string())?
            .is_none()
    {
        return Err(
            "existing store is not sealed for fixed voters; explicit migration required".into(),
        );
    }
    let node = ClusterNode::new_fixed(
        ClusterConfig {
            node_id: config.node_id,
            peers,
            peer_addrs,
            advertise_addr: Some(config.advertise_addr),
            cluster_port: config.cluster_port,
            ..Default::default()
        },
        db,
    )?;
    node.start().await;
    let mut source = PulseSource::start().await?;
    let mut owner: Option<Replicated> = None;
    let mut running: JoinSet<(
        u64,
        mcp_js_scheduler_core::Invocation,
        mcp_js_scheduler_core::Outcome,
    )> = JoinSet::new();
    let mut cancelling: JoinSet<(
        mcp_js_scheduler_runner::scheduled_protocol::Dispatch,
        mcp_js_scheduler_core::Outcome,
    )> = JoinSet::new();
    let mut cancellation_pending = std::collections::HashSet::new();
    let mut input = BufReader::new(tokio::io::stdin()).lines();
    let mut input_open = true;
    let mut stopping = false;
    loop {
        if stopping && running.is_empty() {
            break;
        }
        tokio::select! {
            Some(result) = cancelling.join_next(), if !cancelling.is_empty() => {
                if let Ok((dispatch, outcome)) = result {
                    cancellation_pending.remove(&dispatch.occurrence_id);
                    if let Some(active) = owner.as_mut()
                        && let Err(error) = active.finish_cancellation(&dispatch, outcome, Utc::now()).await {
                        eprintln!("cancellation result uncertain: {error}"); owner = None;
                    }
                }
            }
            Some((request,reply))=admission_rx.recv(), if !stopping => {
                let result=match owner.as_mut() {
                    Some(active)=>active.admit(request,Utc::now()).await.map_err(|e|(axum::http::StatusCode::SERVICE_UNAVAILABLE,e)),
                    None=>Err((axum::http::StatusCode::CONFLICT,"no acquired owner".into())),
                };
                let _=reply.send(result);
            }
            _ = tokio::signal::ctrl_c(), if !stopping => { stopping = true; }
            completed = running.join_next(), if !running.is_empty() => {
                if let Some(Ok((term, invocation, outcome))) = completed
                    && let Some(active) = owner.as_mut().filter(|active| active.term() == term)
                    && active.view().occurrences().iter().any(|o| o.id == invocation.occurrence_id
                        && matches!(o.status, mcp_js_scheduler_core::Status::Running { attempt }
                            | mcp_js_scheduler_core::Status::Cancelling { attempt } if attempt == invocation.attempt))
                    && let Err(error) = active.finish(&invocation, outcome, Utc::now()).await {
                    eprintln!("completion uncertain: {error}"); owner = None;
                }
            }
            line = input.next_line(), if input_open && !stopping => {
                match line.map_err(|e| e.to_string())? {
                    None => input_open = false,
                    Some(line) => {
                        let response = match serde_json::from_str::<Command>(&line) {
                            Err(error) => Err(error.to_string()),
                            Ok(Command::Status) => {
                                let status=node.status().await;
                                let acquired=status.role==Role::Leader && owner.as_ref().is_some_and(|active|active.is_valid() && active.term()==status.term);
                                Ok(serde_json::json!({"cluster":status,"owner_acquired":acquired}))
                            },
                            Ok(command) => match owner.as_mut() {
                                None => Err("no acquired leader; send command to current leader".into()),
                                Some(active) => match command {
                                    Command::Status => unreachable!(),
                                    Command::List => Ok(serde_json::json!({"schedules": active.view().schedules(), "occurrences": active.view().occurrences(), "stale_local_view": true})),
                                    Command::Create { definition } => active.create(definition, Utc::now()).await.map(|id| serde_json::json!({"id":id})),
                                    Command::Disable { id } => active.disable(id).await.map(|_| serde_json::json!({"ok":true})),
                                    Command::Cancel { occurrence_id } => active.request_cancel(occurrence_id).await.map(|_| serde_json::json!({"ok":true,"cancellation_requested":true})),
                                    Command::Delete { id } => active.delete(id).await.map(|_| serde_json::json!({"ok":true})),
                                    Command::AcknowledgeStopped { occurrence_id, dispatcher_and_work_quiesced: true } => active.acknowledge_stopped(occurrence_id).await.map(|_| serde_json::json!({"ok":true})),
                                    Command::AcknowledgeStopped { .. } => Err("must establish old dispatcher cannot submit and in-flight work is resolved".into()),
                                }
                            }
                        };
                        println!("{}", match response { Ok(value) => value, Err(error) => serde_json::json!({"error":error}) });
                    }
                }
            }
            pulse = source.recv(), if !stopping => {
                pulse?;
                let status = node.status().await;
                if status.role != Role::Leader { owner = None; continue; }
                if owner.as_ref().is_none_or(|active| active.term() != status.term) {
                    match Replicated::acquire(node.clone(), config.limits.clone(), &config.schedules, contract.clone(), Utc::now()).await {
                        Ok(active) => owner = Some(active),
                        Err(error) => { eprintln!("acquisition unavailable: {error}"); continue; }
                    }
                }
                let active = owner.as_mut().unwrap();
                // Cleanup remains available after grant expiry. Every owner
                // retries durable cancellation intent; ambiguous results retain capacity.
                for dispatch in active.cancellations() {
                    if cancellation_pending.insert(dispatch.occurrence_id) {
                        let executor = executor.clone();
                        cancelling.spawn(async move {
                            let outcome = executor.cancel(dispatch.clone()).await;
                            (dispatch, outcome)
                        });
                    }
                }
                if Utc::now() >= config.remote.expires_at { continue; }
                if let Err(error) = active.observe(Utc::now()).await { eprintln!("observation uncertain: {error}"); owner = None; continue; }
                loop {
                    match active.claim_for_worker(Utc::now(), &config.remote).await {
                        Ok(Some((invocation,dispatch))) => {
                            let executor = executor.clone(); let term = active.term();
                            running.spawn(async move { let outcome = executor.execute_authorized(dispatch).await; (term, invocation, outcome) });
                        }
                        Ok(None) => break,
                        Err(error) => { eprintln!("claim unavailable: {error}"); owner = None; break; }
                    }
                }
            }
        }
    }
    source.shutdown().await?;
    admission_server.abort();
    node.shutdown();
    Ok(())
}

type AdmissionCall = (
    mcp_js_scheduler_runner::scheduled_protocol::Admission,
    tokio::sync::oneshot::Sender<
        Result<
            mcp_js_scheduler_runner::scheduled_protocol::Permit,
            (axum::http::StatusCode, String),
        >,
    >,
);
async fn admission_handler(
    axum::extract::State((token, tx)): axum::extract::State<(
        String,
        tokio::sync::mpsc::Sender<AdmissionCall>,
    )>,
    headers: axum::http::HeaderMap,
    axum::Json(request): axum::Json<mcp_js_scheduler_runner::scheduled_protocol::Admission>,
) -> (axum::http::StatusCode, axum::Json<serde_json::Value>) {
    use axum::http::StatusCode;
    if headers.get("authorization").and_then(|h| h.to_str().ok())
        != Some(format!("Bearer {token}").as_str())
    {
        return (
            StatusCode::UNAUTHORIZED,
            axum::Json(serde_json::json!({"error":"unauthorized"})),
        );
    }
    let (reply, rx) = tokio::sync::oneshot::channel();
    if tx.send((request, reply)).await.is_err() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(serde_json::json!({"error":"runner stopped"})),
        );
    }
    match rx.await {
        Ok(Ok(permit)) => (
            StatusCode::CREATED,
            axum::Json(serde_json::to_value(permit).unwrap()),
        ),
        Ok(Err((status, error))) => (status, axum::Json(serde_json::json!({"error":error}))),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(serde_json::json!({"error":"admission uncertain"})),
        ),
    }
}
