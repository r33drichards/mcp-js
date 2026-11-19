//! Real-I/O scenario driver: the controller alone may crash or bounce hosts.
//! Observer and host exchange serialized messages, never sled/engine handles.
use super::*;
use std::cell::Cell;
use std::collections::VecDeque;

#[derive(Clone)]
enum HostAction {
    Crash(String),
    Bounce(String),
}
#[derive(Default)]
struct Control {
    pending: VecDeque<(u64, HostAction)>,
    done: u64,
    next: u64,
}

async fn control_host(control: &Rc<RefCell<Control>>, action: HostAction) {
    let seq = {
        let mut c = control.borrow_mut();
        c.next += 1;
        let seq = c.next;
        c.pending.push_back((seq, action));
        seq
    };
    while control.borrow().done < seq {
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

pub(super) fn run_scenario(sc: &Scenario) -> ScenarioResult {
    let control = Rc::new(RefCell::new(Control::default()));
    let finished = Rc::new(Cell::new(false));
    let max_fault = sc.faults.iter().map(Fault::at_ms).max().unwrap_or(0);
    let total_secs = 60 + max_fault / 1000 + sc.recovery.deadline_ms / 1000;
    let mut sim = turmoil::Builder::new()
        .rng_seed(sc.seed)
        .enable_random_order()
        .min_message_latency(Duration::from_millis(1))
        .max_message_latency(Duration::from_millis(3))
        .simulation_duration(Duration::from_secs(total_secs))
        .build();
    for (i, name) in ["n0", "n1", "n2"].into_iter().enumerate() {
        let peers: Vec<_> = ["n0", "n1", "n2"]
            .into_iter()
            .filter(|p| *p != name)
            .map(|p| format!("{p}:{PORT}"))
            .collect();
        let peer_addrs = ["n0", "n1", "n2"]
            .into_iter()
            .filter(|p| *p != name)
            .map(|p| (p.to_owned(), format!("{p}:{PORT}")))
            .collect();
        let timeout = [70, 180, 300][i];
        let cfg = ClusterConfig {
            node_id: name.to_owned(),
            peers,
            peer_addrs,
            cluster_port: PORT,
            advertise_addr: Some(format!("{name}:{PORT}")),
            heartbeat_interval: Duration::from_millis(20),
            election_timeout_min: Duration::from_millis(timeout),
            election_timeout_max: Duration::from_millis(timeout),
            learner: false,
        };
        sim.host(name, move || {
            let cfg = cfg.clone();
            async move {
                // Turmoil gives each named host its own persistent simulated FS.
                let db = sled::Config::new().path("/scenario-db")
                    .flush_every_ms(None).open()?;
                let node = ClusterNode::new(cfg, db.clone()).with_peer_rpc(Arc::new(SimRpc));
                let host = Arc::new(ScenarioHost::new(node.clone(), db));
                let raft = TcpListener::bind(("0.0.0.0", PORT)).await?;
                let observer = TcpListener::bind(("0.0.0.0", HOST_PORT)).await?;
                node.start_background_tasks().expect("start election and heartbeat loops");
                let mut connections = JoinSet::new();
                loop {
                    tokio::select! {
                        incoming = raft.accept() => {
                            let (stream, _) = incoming?;
                            let node = node.clone();
                            connections.spawn(async move {
                                server::cluster::serve_cluster_connection(node, stream)
                                    .await.expect("raft HTTP connection");
                            });
                        }
                        incoming = observer.accept() => {
                            let (stream, _) = incoming?;
                            let host = host.clone();
                            connections.spawn(async move {
                                host.serve_request(stream).await.expect("host RPC connection");
                            });
                        }
                        done = connections.join_next(), if !connections.is_empty() => {
                            if let Some(Err(panic)) = done { panic!("host handler panicked: {panic}"); }
                        }
                    }
                }
            }
        });
    }
    let scenario = sc.clone();
    let observer_control = control.clone();
    let observer_finished = finished.clone();
    sim.client("observer", async move {
        let result = run_client(&scenario, observer_control).await;
        store_result(result);
        observer_finished.set(true);
        Ok(())
    });
    for _ in 0..200_000 {
        let next = { control.borrow_mut().pending.pop_front() };
        if let Some((seq, action)) = next {
            match action {
                HostAction::Crash(name) => sim.crash(name.as_str()),
                HostAction::Bounce(name) => sim.bounce(name.as_str()),
            }
            control.borrow_mut().done = seq;
        }
        if finished.get() {
            return take_result();
        }
        if let Err(e) = sim.step() {
            return blank(Err(format!("real-I/O simulation failed: {e}")));
        }
    }
    blank(Err(
        "real-I/O scenario exceeded controller step budget".into()
    ))
}

async fn current_leader() -> Option<(String, u64)> {
    for name in ["n0", "n1", "n2"] {
        if let Ok((Role::Leader, term)) = ScenarioHostClient::new(name).status().await {
            return Some((name.to_owned(), term));
        }
    }
    None
}

async fn publish(
    sc: &Scenario,
    client: &ScenarioHostClient,
    iteration: usize,
    op_id: u64,
    acked: &mut Vec<String>,
    attempted: &mut Vec<String>,
    unknown: &mut Vec<String>,
    failed: &mut Vec<String>,
    model: &mut Model,
) -> Outcome {
    let code = match &sc.workload.op {
        Op::RunJs { program, .. } => program.render_js(iteration),
        Op::RunJsUnmodeled { source, .. } => source.clone(),
    };
    let unmodeled = matches!(&sc.workload.op, Op::RunJsUnmodeled { .. });
    let result = match client.run_js(&code).await {
        Ok(result) => result,
        Err(error) => JsResult::Failed(error),
    };
    let head = match result {
        JsResult::Head(head) => head,
        JsResult::JsException(error) if unmodeled => return Outcome::JsException { error },
        JsResult::TimedOut if unmodeled => {
            return Outcome::ResourceLimit {
                error: "engine timed out".into(),
            };
        }
        JsResult::JsException(error) | JsResult::Failed(error) => {
            failed.push(format!("op{op_id}:{error}"));
            return Outcome::Failed { error };
        }
        JsResult::TimedOut => {
            failed.push(format!("op{op_id}:timed_out"));
            return Outcome::Failed {
                error: "timed_out".into(),
            };
        }
    };
    attempted.push(head.clone());
    let Some(ca) = parse_ca_hex(&head) else {
        failed.push(format!("op{op_id}:bad-ca-hex"));
        return Outcome::Failed {
            error: "invalid JS snapshot head".into(),
        };
    };
    // A failed pre-CAS read cannot be a lost CAS acknowledgement.
    let expect = match client.resolve("main").await {
        Ok(expect) => expect,
        Err(error) => {
            failed.push(format!("op{op_id}:resolve:{error}"));
            return Outcome::Failed { error };
        }
    };
    if expect.as_ref() == Some(&ca) {
        return Outcome::Conflict;
    }
    match client.cas("main", expect, ca).await {
        Ok(true) => {
            acked.push(head.clone());
            if let Op::RunJs { program, .. } = &sc.workload.op {
                for step in &program.0 {
                    let Step::Write { path, bytes } = step;
                    model.insert(
                        path.clone(),
                        bytes.replace("{i}", &iteration.to_string()).into_bytes(),
                    );
                }
            }
            Outcome::Acked { head }
        }
        Ok(false) => Outcome::Conflict,
        Err(_) => {
            unknown.push(format!("op{op_id}"));
            Outcome::Unknown
        }
    }
}

async fn run_client(sc: &Scenario, control: Rc<RefCell<Control>>) -> ScenarioResult {
    let start = tokio::time::Instant::now();
    let mut events = Vec::new();
    let mut acked = Vec::new();
    let mut unknown = Vec::new();
    let mut failed = Vec::new();
    let mut attempted = Vec::new();
    let mut model: Model = BTreeMap::new();
    let mut crashed = Vec::new();
    tokio::time::sleep(Duration::from_millis(600)).await;
    let Some((leader, term)) = current_leader().await else {
        return blank(Err("no leader elected within 600ms".into()));
    };
    events.push(ScenarioEvent::Election {
        at_ms: now_ms(start),
        leader: leader.clone(),
        term,
    });
    let op_node = sc.workload.op.resolve_node(&leader);
    let client = ScenarioHostClient::new(&op_node);
    let mut faults = sc.faults.clone();
    faults.sort_by_key(Fault::at_ms);
    let mut ops_done = 0usize;
    let mut op_id = 0u64;
    let mut op_start = start + Duration::from_millis(600);
    loop {
        let next_fault = faults.first().map(Fault::at_ms);
        let next_op = (ops_done < sc.workload.count).then_some(op_start);
        let fault_first = match (next_fault, next_op) {
            (None, None) => break,
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (Some(ft), Some(ot)) => ft <= ot.duration_since(start).as_millis() as u64,
        };
        if fault_first {
            let fault = faults.remove(0);
            tokio::time::sleep_until(start + Duration::from_millis(fault.at_ms())).await;
            let at = now_ms(start);
            match fault {
                Fault::Crash { node, .. } => {
                    let target = match &node {
                        CrashTarget::Node(name) => name.clone(),
                        CrashTarget::CurrentLeader => current_leader()
                            .await
                            .map(|(name, _)| name)
                            .unwrap_or_else(|| leader.clone()),
                    };
                    control_host(&control, HostAction::Crash(target.clone())).await;
                    crashed.push(target.clone());
                    events.push(ScenarioEvent::Crash {
                        at_ms: at,
                        node: target.clone(),
                        target: format!("{node:?}={target}"),
                    });
                }
                Fault::HealAll { .. } => {
                    for a in 0..HOSTS.len() {
                        for b in a + 1..HOSTS.len() {
                            turmoil::repair(HOSTS[a], HOSTS[b]);
                        }
                    }
                    events.push(ScenarioEvent::HealAll { at_ms: at });
                }
                Fault::Partition { groups, .. } => {
                    let chosen = groups
                        .iter()
                        .position(|g| g.iter().any(|h| h == "current_leader"));
                    let mut resolved = Vec::<Vec<String>>::new();
                    for (gi, group) in groups.iter().enumerate() {
                        let mut names = Vec::new();
                        for name in group {
                            let name = if name == "current_leader" {
                                leader.clone()
                            } else {
                                name.clone()
                            };
                            if chosen.is_some_and(|i| i != gi && name == leader) {
                                continue;
                            }
                            if !names.contains(&name) {
                                names.push(name);
                            }
                        }
                        resolved.push(names);
                    }
                    let named: Vec<&str> = resolved.iter().flatten().map(String::as_str).collect();
                    let group_of =
                        |name: &str| resolved.iter().position(|g| g.iter().any(|n| n == name));
                    for a in 0..HOSTS.len() {
                        for b in a + 1..HOSTS.len() {
                            let (x, y) = (HOSTS[a], HOSTS[b]);
                            if named.contains(&x)
                                && named.contains(&y)
                                && group_of(x) != group_of(y)
                            {
                                turmoil::partition(x, y);
                                events.push(ScenarioEvent::Partition {
                                    at_ms: at,
                                    a: x.into(),
                                    b: y.into(),
                                });
                            }
                        }
                    }
                }
            }
        } else {
            tokio::time::sleep_until(op_start).await;
            let iteration = ops_done;
            op_id += 1;
            events.push(ScenarioEvent::Invoke {
                at_ms: now_ms(start),
                op_id,
                node: op_node.clone(),
            });
            let outcome = publish(
                sc,
                &client,
                iteration,
                op_id,
                &mut acked,
                &mut attempted,
                &mut unknown,
                &mut failed,
                &mut model,
            )
            .await;
            events.push(ScenarioEvent::Respond {
                at_ms: now_ms(start),
                op_id,
                node: op_node.clone(),
                outcome,
            });
            ops_done += 1;
            op_start = tokio::time::Instant::now() + Duration::from_millis(sc.workload.every_ms);
        }
    }
    // Recovery is controller-driven: no soft shutdown or cross-host DB opener.
    if sc.recovery.heal_all {
        for a in 0..HOSTS.len() {
            for b in a + 1..HOSTS.len() {
                turmoil::repair(HOSTS[a], HOSTS[b]);
            }
        }
        events.push(ScenarioEvent::HealAll {
            at_ms: now_ms(start),
        });
    }
    if sc.recovery.restart_crashed {
        for name in &crashed {
            control_host(&control, HostAction::Bounce(name.clone())).await;
            events.push(ScenarioEvent::Restart {
                at_ms: now_ms(start),
                node: name.clone(),
            });
        }
    }
    tokio::time::sleep(Duration::from_millis(600)).await;

    // Read the old acknowledged content after a real owner bounce, before any
    // new JS executes. A recovered head alone is not recovered snapshot bytes.
    let mut recovered_bytes = true;
    if crashed.contains(&op_node) && !acked.is_empty() {
        let old_head = acked.last().unwrap().clone();
        for (path, expected) in &model {
            if client
                .read_file(&old_head, path)
                .await
                .ok()
                .flatten()
                .as_deref()
                != Some(expected.as_slice())
            {
                recovered_bytes = false;
            }
        }
        events.push(ScenarioEvent::Assert {
            at_ms: now_ms(start),
            name: "RecoveredPreCrashSnapshotBytes".into(),
            passed: recovered_bytes,
        });
    }
    let prior_head = acked.last().cloned();
    let mut fresh_acked = false;
    if sc.asserts.contains(&Assertion::FreshWriteSucceeds) {
        for retry in 0..5 {
            if retry > 0 {
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            let iteration = sc.workload.count + retry;
            op_id += 1;
            events.push(ScenarioEvent::Invoke {
                at_ms: now_ms(start),
                op_id,
                node: op_node.clone(),
            });
            let outcome = publish(
                sc,
                &client,
                iteration,
                op_id,
                &mut acked,
                &mut attempted,
                &mut unknown,
                &mut failed,
                &mut model,
            )
            .await;
            if let Outcome::Acked { head } = &outcome {
                fresh_acked = prior_head.as_ref() != Some(head);
            }
            events.push(ScenarioEvent::Respond {
                at_ms: now_ms(start),
                op_id,
                node: op_node.clone(),
                outcome,
            });
            if fresh_acked {
                break;
            }
        }
    }
    let mut final_heads: BTreeMap<String, Option<String>> = BTreeMap::new();
    let deadline = tokio::time::Instant::now() + Duration::from_millis(sc.recovery.deadline_ms);
    let mut converged = false;
    loop {
        final_heads.clear();
        for name in ["n0", "n1", "n2"] {
            let head = ScenarioHostClient::new(name)
                .resolve("main")
                .await
                .ok()
                .flatten()
                .map(|ca| to_hex(&ca));
            final_heads.insert(name.to_owned(), head);
        }
        let head = final_heads.values().next().and_then(Option::as_ref);
        let equal = final_heads.values().all(|other| other.as_ref() == head);
        let legal =
            head.is_none() && acked.is_empty() || head.is_some_and(|h| attempted.contains(h));
        if equal && legal {
            converged = true;
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let mut status = if !recovered_bytes {
        Err("pre-crash acknowledged snapshot bytes were not recovered".into())
    } else if !converged {
        Err(format!(
            "replicas did not converge: heads={final_heads:?} acked={acked:?} unknown={unknown:?}"
        ))
    } else {
        Ok("all assertions passed".to_owned())
    };
    for assertion in &sc.asserts {
        let passed = match assertion {
            Assertion::FreshWriteSucceeds => fresh_acked && recovered_bytes,
            Assertion::NoUnexpectedJsFailure => failed.is_empty(),
            Assertion::ReplicasConvergeAfterRecovery => converged,
            Assertion::AckedCommitsInLegalHistory => {
                let final_head = final_heads.values().next().cloned().flatten();
                match client.log("main").await {
                    Ok(entries) => {
                        let tos: Vec<String> =
                            entries.iter().map(|entry| to_hex(&entry.to)).collect();
                        acked
                            .iter()
                            .all(|head| tos.contains(head) || final_head.as_deref() == Some(head))
                    }
                    Err(_) => false,
                }
            }
            Assertion::PublishedSnapshotMatchesModel => {
                match final_heads.values().next().cloned().flatten() {
                    None => model.is_empty(),
                    Some(head) => {
                        let mut matches = true;
                        for (path, expected) in &model {
                            if client
                                .read_file(&head, path)
                                .await
                                .ok()
                                .flatten()
                                .as_deref()
                                != Some(expected.as_slice())
                            {
                                matches = false;
                            }
                        }
                        matches
                    }
                }
            }
        };
        events.push(ScenarioEvent::Assert {
            at_ms: now_ms(start),
            name: format!("{assertion:?}"),
            passed,
        });
        if !passed {
            status = Err(format!("assertion failed: {assertion:?}"));
        }
    }
    ScenarioResult {
        status,
        events,
        acked,
        unknown,
        failed,
        final_heads,
        model,
        artifact_dir: None,
    }
}
