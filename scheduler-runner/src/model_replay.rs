//! Deterministic TLC trace adapter. Production Core/Replicated/Raft transitions;
//! explicit in-memory transport and worker semantics, no sockets or wall sleeps.
use super::{Replicated, cluster::*};
use chrono::{TimeZone, Utc};
use futures::{FutureExt, poll};
use mcp_js_scheduler_core::{Definition, Executor, Invocation, Limits, Outcome, Policy, Result};
use serde_json::Value;
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    task::Poll,
    time::Duration,
};
use tokio::sync::Mutex as AsyncMutex;

type Owner = Arc<AsyncMutex<Replicated>>;
type Claim = Pin<Box<dyn Future<Output = Result<Option<Invocation>>> + Send>>;
#[derive(Default)]
struct ControlledExecutor(Mutex<usize>);
impl Executor for ControlledExecutor {
    fn execute(&self, _: Invocation) -> Pin<Box<dyn Future<Output = Outcome> + Send + '_>> {
        Box::pin(async move {
            *self.0.lock().unwrap() += 1;
            futures::future::pending().await
        })
    }
}
struct Network {
    nodes: Vec<Arc<ClusterNode>>,
    configs: Vec<ClusterConfig>,
    dbs: Vec<sled::Db>,
    up: [bool; 3],
    quorum: bool,
    leader: usize,
    term: u64,
    checkpoint: Option<String>,
}
impl Network {
    async fn new() -> Self {
        let configs: Vec<_> = (0..3)
            .map(|i| {
                let peers: Vec<_> = (0..3)
                    .filter(|j| *j != i)
                    .map(|j| format!("n{}@127.0.0.1:{}", j + 1, 4600 + j))
                    .collect();
                let (peers, peer_addrs) = ClusterConfig::parse_peers(&peers);
                ClusterConfig {
                    node_id: format!("n{}", i + 1),
                    cluster_port: 4600 + i as u16,
                    advertise_addr: Some(format!("127.0.0.1:{}", 4600 + i)),
                    peers,
                    peer_addrs,
                    ..Default::default()
                }
            })
            .collect();
        let dbs: Vec<_> = (0..3)
            .map(|_| sled::Config::new().temporary(true).open().unwrap())
            .collect();
        let nodes = configs
            .iter()
            .zip(&dbs)
            .map(|(c, d)| ClusterNode::new_fixed(c.clone(), d.clone()).unwrap())
            .collect();
        let mut net = Self {
            nodes,
            configs,
            dbs,
            up: [true; 3],
            quorum: true,
            leader: 0,
            term: 1,
            checkpoint: None,
        };
        net.elect(0, 1).await;
        net
    }
    async fn elect(&mut self, node: usize, term: u64) {
        let state = self.nodes[node].state.read().await;
        let request = RequestVoteRequest {
            term,
            candidate_id: self.configs[node].node_id.clone(),
            last_log_index: state.last_log_index(),
            last_log_term: state.last_log_term(),
        };
        drop(state);
        let mut votes = 0;
        for i in 0..3 {
            if self.up[i]
                && self.nodes[i]
                    .handle_request_vote(request.clone())
                    .await
                    .vote_granted
            {
                votes += 1;
            }
        }
        assert!(votes >= 2, "model election must obtain real handler quorum");
        self.nodes[node].sim_install_elected_leader(term).await;
        self.leader = node;
        self.term = term;
    }
    async fn replicate(&mut self) {
        if !self.quorum || !self.up[self.leader] {
            return;
        }
        // Reversed delivery order and duplicate AppendEntries exercise real
        // idempotence; the second round propagates the new commit index.
        for _ in 0..2 {
            let state = self.nodes[self.leader].state.read().await;
            let request = AppendEntriesRequest {
                term: state.current_term,
                leader_id: self.configs[self.leader].node_id.clone(),
                prev_log_index: 0,
                prev_log_term: 0,
                entries: state.log.clone(),
                leader_commit: state.commit_index,
                peer_addrs: None,
                learner_addrs: None,
            };
            drop(state);
            for peer in (0..3).rev() {
                if peer == self.leader || !self.up[peer] {
                    continue;
                }
                let first = self.nodes[peer]
                    .handle_append_entries(request.clone())
                    .await;
                // Lose the first reply, then deliver a duplicate request/reply.
                assert!(first.success);
                let response = self.nodes[peer]
                    .handle_append_entries(request.clone())
                    .await;
                let addr = self.configs[peer].advertise_addr.as_ref().unwrap();
                self.nodes[self.leader]
                    .sim_response(addr, request.term, request.entries.len() as u64, response)
                    .await;
            }
            self.nodes[self.leader].sim_commit().await;
        }
        self.refresh().await;
    }
    async fn refresh(&mut self) {
        if let Some(value) = self.nodes[self.leader]
            .get("scheduler/state/v1")
            .await
            .unwrap()
        {
            let envelope: Value = serde_json::from_str(&value).unwrap();
            self.checkpoint = Some(envelope["checkpoint"].as_str().unwrap().into());
        }
    }
    async fn drive<T>(&mut self, future: impl Future<Output = T>) -> T {
        tokio::pin!(future);
        for _ in 0..32 {
            if let Poll::Ready(result) = poll!(future.as_mut()) {
                self.refresh().await;
                return result;
            }
            self.replicate().await;
            tokio::time::advance(Duration::from_millis(50)).await;
        }
        panic!("operation did not complete with deterministic deliveries");
    }
    fn receipt(&self) -> String {
        let Some(bytes) = &self.checkpoint else {
            return "Pending".into();
        };
        let state: Value = serde_json::from_str(bytes).unwrap();
        let Some(occurrence) = state["occurrences"].as_object().unwrap().values().next() else {
            return "Pending".into();
        };
        let status = occurrence["status"]
            .as_object()
            .unwrap()
            .keys()
            .next()
            .unwrap();
        if status == "Succeeded" {
            "Completed".into()
        } else {
            status.clone()
        }
    }
}
fn now(t: u64) -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 1, 1, 0, t as u32, 0).unwrap()
}
fn definition() -> Definition {
    Definition {
        name: None,
        code: "1+1".into(),
        cron: "* * * * *".into(),
        timezone: "UTC".into(),
        policy: Policy::default(),
    }
}

async fn replay(trace: &Value) -> std::result::Result<(), String> {
    let mut net = Network::new().await;
    let mut owners: Vec<Option<Owner>> = vec![None, None];
    let mut claims: Vec<Option<Claim>> = vec![None, None];
    let mut tickets: Vec<Option<(Invocation, u64)>> = vec![None, None];
    let mut messages: Vec<Value> = vec![];
    let mut work: Vec<Value> = vec![];
    let executor = ControlledExecutor::default();
    let mut admissions = 0;
    let mut logical = 0;
    for expected in trace["states"].as_array().unwrap().iter().skip(1) {
        let event = &expected["event"];
        let op = event["op"].as_str().unwrap();
        let n = event["node"].as_u64().unwrap().saturating_sub(1) as usize;
        let arg = event["arg"].as_u64().unwrap();
        match op {
            "tick" => {
                logical = arg;
                if let Some(owner) = &owners[net.leader] {
                    let owner = owner.clone();
                    net.drive(async { owner.lock().await.observe(now(logical)).await })
                        .await
                        .unwrap();
                }
            }
            "acquire" => {
                let node = net.nodes[n].clone();
                let acquired = net
                    .drive(Replicated::acquire(
                        node,
                        Limits::default(),
                        &[definition()],
                        "model-grant".into(),
                        now(0),
                    ))
                    .await
                    .unwrap();
                let owner = Arc::new(AsyncMutex::new(acquired));
                if logical > 0 {
                    let copy = owner.clone();
                    net.drive(async { copy.lock().await.observe(now(logical)).await })
                        .await
                        .unwrap();
                }
                owners[n] = Some(owner);
            }
            "propose" => {
                let owner = owners[n].as_ref().unwrap().clone();
                let mut future: Claim =
                    Box::pin(async move { owner.lock().await.claim(now(logical)).await });
                if !matches!(poll!(future.as_mut()), Poll::Pending) {
                    return Err("claim did not await quorum".into());
                }
                claims[n] = Some(future);
            }
            "commit" => net.replicate().await,
            "ack" => {
                tokio::time::advance(Duration::from_millis(50)).await;
                let mut claim = claims[n].take().ok_or("missing claim future")?;
                match poll!(claim.as_mut()) {
                    Poll::Ready(Ok(Some(invocation))) => tickets[n] = Some((invocation, arg)),
                    Poll::Ready(Err(_)) => (),
                    other => return Err(format!("unexpected claim result: {other:?}")),
                }
            }
            "lose_ack" => {
                claims[n] = None;
                owners[n] = None;
            }
            "send" => {
                let (invocation, term) = tickets[n].take().ok_or("no real dispatch ticket")?;
                assert!(executor.execute(invocation).now_or_never().is_none());
                messages.push(serde_json::json!({"kind":"post","node":n+1,"term":term}));
            }
            "deliver_post" => {
                work.push(messages.remove(arg as usize - 1));
                admissions += 1;
            }
            "finish" => {
                let mut message = work.remove(arg as usize - 1);
                message["kind"] = Value::String("done".into());
                messages.push(message);
            }
            "deliver_done" | "deliver_done_lost_ack" => {
                let message = messages.remove(arg as usize - 1);
                if let Some(owner) = &owners[n] {
                    let owner = owner.clone();
                    let records = owner.lock().await.view().occurrences();
                    if let Some(record) = records.first() {
                        let invocation = Invocation {
                            occurrence_id: record.id,
                            schedule_id: record.schedule_id,
                            due_at: record.due_at,
                            attempt: 1,
                            code: "1+1".into(),
                        };
                        let valid = message["term"].as_u64().unwrap() == net.term
                            && n == net.leader
                            && net.quorum
                            && net.receipt() == "Running";
                        if op == "deliver_done_lost_ack" && valid {
                            let mut completion = Box::pin(async {
                                owner
                                    .lock()
                                    .await
                                    .finish(&invocation, Outcome::Succeeded, now(logical))
                                    .await
                            });
                            assert!(matches!(poll!(completion.as_mut()), Poll::Pending));
                            net.replicate().await;
                            drop(completion); // Durable receipt, caller never observes commit ACK.
                            assert!(owner.lock().await.observe(now(logical)).await.is_err());
                            owners[n] = None;
                        } else {
                            let result = net
                                .drive(async {
                                    owner
                                        .lock()
                                        .await
                                        .finish(&invocation, Outcome::Succeeded, now(logical))
                                        .await
                                })
                                .await;
                            if valid {
                                result.map_err(|e| format!("completion rejected: {e}"))?;
                            } else if result.is_ok() {
                                return Err("stale completion accepted".into());
                            }
                        }
                    }
                }
            }

            "drop" => {
                messages.remove(arg as usize - 1);
            }
            "duplicate" => messages.push(messages[arg as usize - 1].clone()),
            "partition" => net.quorum = false,
            "heal" => net.quorum = true,
            "elect" => net.elect(n, arg).await,
            "crash" => {
                net.nodes[n].shutdown();
                net.up[n] = false;
                owners[n] = None;
                claims[n] = None;
                tickets[n] = None;
            }
            "restart" => {
                net.nodes[n] =
                    ClusterNode::new_fixed(net.configs[n].clone(), net.dbs[n].clone()).unwrap();
                net.up[n] = true;
            }
            other => return Err(format!("unmapped action {other}")),
        }
        let actual_tickets: Vec<u64> = tickets
            .iter()
            .map(|ticket| ticket.as_ref().map_or(0, |(_, term)| *term))
            .collect();
        let actual = serde_json::json!({"receipt":net.receipt(),"posts":*executor.0.lock().unwrap(),"admissions":admissions,"tickets":actual_tickets,"net":messages,"work":work,"now":logical});
        for field in [
            "receipt",
            "posts",
            "admissions",
            "tickets",
            "net",
            "work",
            "now",
        ] {
            if actual[field] != expected[field] {
                return Err(format!(
                    "step {} {op}: {field}: actual={} model={}",
                    expected["step"], actual[field], expected[field]
                ));
            }
        }
    }
    Ok(())
}
#[tokio::test(start_paused = true)]
async fn replay_tlc_generated_states_against_production_transitions() {
    let directory = std::env::var_os("SCHEDULER_MODEL_TRACES")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../specs/runner/generated")
        });
    let mut paths: Vec<_> = std::fs::read_dir(directory)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.extension().is_some_and(|e| e == "json")
                && !p.to_string_lossy().ends_with(".tlc.json")
                && p.file_name().unwrap() != "summary.json"
        })
        .collect();
    paths.sort();
    assert!(paths.len() >= 26, "generated traces missing");
    let mut actions = std::collections::BTreeSet::new();
    for path in paths {
        let trace: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        for state in trace["states"].as_array().unwrap() {
            actions.insert(state["event"]["op"].as_str().unwrap().to_owned());
        }
        let result = replay(&trace).await;
        if trace["mutation"].is_null() {
            assert!(result.is_ok(), "{}: {result:?}", path.display());
        } else {
            let error = result.expect_err("production accepted mutant trace");
            let expected = match trace["mutation"].as_str().unwrap() {
                "replay_unknown" => "receipt: actual=\"Unknown\" model=\"Pending\"",
                "index_ack" => "tickets:",
                other => panic!("unknown mutation {other}"),
            };
            assert!(
                error.contains(expected),
                "{}: unrelated divergence: {error}",
                path.display()
            );
        }
    }
    eprintln!("TLC action coverage: {actions:?}");
    assert_eq!(actions.len(), 19); // init + all 18 executable actions
}

#[tokio::test(start_paused = true)]
async fn deterministic_flush_failure_cannot_release_generated_claim() {
    let mut net = Network::new().await;
    let node = net.nodes[0].clone();
    let mut owner = net
        .drive(Replicated::acquire(
            node,
            Limits::default(),
            &[definition()],
            "model-grant".into(),
            now(0),
        ))
        .await
        .unwrap();
    net.drive(owner.observe(now(1))).await.unwrap();
    net.nodes[0].sim_storage_failure(true);
    assert!(owner.claim(now(1)).await.is_err());
    assert!(owner.claim(now(1)).await.is_err());
}

#[path = "admission_replay.rs"]
mod admission_replay;
