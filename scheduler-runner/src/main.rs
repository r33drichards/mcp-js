use chrono::Utc;
use mcp_js_scheduler_core::{Definition, Executor, Limits, PulseSource};
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
    List,
    Create { definition: Definition },
    Disable { id: u64 },
    Delete { id: u64 },
    AcknowledgeStopped { occurrence_id: u64 },
}

#[tokio::main]
async fn main() -> Result<(), String> {
    let path = std::env::args()
        .nth(1)
        .ok_or("usage: mcp-js-scheduler-runner CONFIG.json")?;
    let config: Config = serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let executor = Arc::new(RemoteExecutor::new(config.remote.clone())?);
    // All replicas must use the same grant reference/expiry, limits and initial definitions.
    let contract = serde_json::to_string(&Contract {
        limits: &config.limits,
        schedules: &config.schedules,
        remote: &config.remote,
    })
    .map_err(|e| e.to_string())?;
    let (peers, peer_addrs) = ClusterConfig::parse_peers(&config.peers);
    let node = ClusterNode::new_fixed(
        ClusterConfig {
            node_id: config.node_id,
            peers,
            peer_addrs,
            advertise_addr: Some(config.advertise_addr),
            cluster_port: config.cluster_port,
            ..Default::default()
        },
        sled::open(config.data_dir).map_err(|e| e.to_string())?,
    )?;
    node.start().await;
    let mut source = PulseSource::start().await?;
    let mut owner: Option<Replicated> = None;
    let mut running = JoinSet::new();
    let mut input = BufReader::new(tokio::io::stdin()).lines();
    let mut input_open = true;
    let mut stopping = false;
    loop {
        if stopping && running.is_empty() {
            break;
        }
        tokio::select! {
            _ = tokio::signal::ctrl_c(), if !stopping => { stopping = true; }
            completed = running.join_next(), if !running.is_empty() => {
                if let Some(Ok((term, invocation, outcome))) = completed
                    && let Some(active) = owner.as_mut().filter(|active| active.term() == term)
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
                            Ok(command) => match owner.as_mut() {
                                None => Err("no acquired leader; send command to current leader".into()),
                                Some(active) => match command {
                                    Command::List => Ok(serde_json::json!({"schedules": active.view().schedules(), "occurrences": active.view().occurrences(), "stale_local_view": true})),
                                    Command::Create { definition } => active.create(definition, Utc::now()).await.map(|id| serde_json::json!({"id":id})),
                                    Command::Disable { id } => active.disable(id).await.map(|_| serde_json::json!({"ok":true})),
                                    Command::Delete { id } => active.delete(id).await.map(|_| serde_json::json!({"ok":true})),
                                    Command::AcknowledgeStopped { occurrence_id } => active.acknowledge_stopped(occurrence_id).await.map(|_| serde_json::json!({"ok":true})),
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
                if Utc::now() >= config.remote.expires_at { continue; }
                let active = owner.as_mut().unwrap();
                if let Err(error) = active.observe(Utc::now()).await { eprintln!("observation uncertain: {error}"); owner = None; continue; }
                loop {
                    match active.claim(Utc::now()).await {
                        Ok(Some(invocation)) => {
                            let executor = executor.clone(); let term = active.term();
                            running.spawn(async move { let outcome = executor.execute(invocation.clone()).await; (term, invocation, outcome) });
                        }
                        Ok(None) => break,
                        Err(error) => { eprintln!("claim unavailable: {error}"); owner = None; break; }
                    }
                }
            }
        }
    }
    source.shutdown().await?;
    node.shutdown();
    Ok(())
}
