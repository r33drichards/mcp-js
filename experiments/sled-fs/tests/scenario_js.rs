//! Opt-in integration: the same JS scenario runner linked against Turmoil-backed sled.
#![cfg(feature = "scenario")]

use engine_cluster_tests::scenario::{
    Assertion, CrashTarget, Fault, Op, Program, Recovery, Scenario, Step, Topology, Workload,
};

#[test]
fn real_sled_reopens_after_crash_and_acknowledges_new_js_write() {
    let scenario = Scenario {
        seed: 42,
        topology: Topology {
            voters: ["n0", "n1", "n2"].map(str::to_owned).to_vec(),
            clients: vec!["observer".into()],
        },
        workload: Workload {
            every_ms: 250,
            count: 1,
            op: Op::RunJs {
                node: "leader".into(),
                program: Program(vec![Step::Write {
                    path: "/data/iteration".into(),
                    bytes: "iteration-{i}".into(),
                }]),
            },
        },
        faults: vec![Fault::Crash {
            at_ms: 900,
            node: CrashTarget::CurrentLeader,
        }],
        recovery: Recovery {
            stop_workload: true,
            heal_all: true,
            restart_crashed: true,
            deadline_ms: 10_000,
        },
        asserts: vec![
            Assertion::FreshWriteSucceeds,
            Assertion::AckedCommitsInLegalHistory,
            Assertion::ReplicasConvergeAfterRecovery,
            Assertion::PublishedSnapshotMatchesModel,
        ],
    };
    let result = scenario.run_and_dump("real-sled-crash-js");
    result.expect_status("all assertions passed");
    assert_eq!(
        result.acked.len(),
        2,
        "old and fresh JS writes need acknowledgments: {:?}",
        result.events
    );
    assert_ne!(result.acked[0], result.acked[1]);
    let elected = result.events.iter().find_map(|e| match e {
        engine_cluster_tests::scenario::ScenarioEvent::Election { leader, .. } => Some(leader),
        _ => None,
    }).unwrap();
    assert!(result.events.iter().any(|e| matches!(e,
        engine_cluster_tests::scenario::ScenarioEvent::Crash { node, .. } if node == elected
    )), "the JS owner must actually crash: {:?}", result.events);
    let ready = result.events.iter().position(|e| matches!(e,
        engine_cluster_tests::scenario::ScenarioEvent::Assert { name, passed: true, .. }
            if name == "FreshReopenGenerationReady"
    )).expect("reopened owner never served its new generation: expected a readiness event");
    let restart = result.events.iter().position(|e| matches!(e,
        engine_cluster_tests::scenario::ScenarioEvent::Restart { node, .. } if node == elected
    )).expect("crashed owner never restarted");
    assert!(ready < restart, "restart was recorded before the reopened host served requests");
}

#[test]
fn execution_registry_reuses_host_owned_persistent_sled() {
    use server::engine::execution::ExecutionRegistry;
    let mut sim = turmoil::Builder::new().build();
    sim.host("n0", || async {
        let config = || sled::Config::new().path("/registry").flush_every_ms(None);
        let db = config().open().unwrap();
        let registry = ExecutionRegistry::from_db(db.clone()).expect("host registry");
        let console = registry.register("first").unwrap();
        console.insert(b"line", b"persisted").unwrap();
        db.flush().unwrap();
        drop(console);
        drop(registry);
        drop(db);
        let reopened = config().open().unwrap();
        assert_eq!(
            reopened
                .open_tree("ex:first")
                .unwrap()
                .get(b"line")
                .unwrap()
                .as_deref(),
            Some(b"persisted".as_slice()),
        );
        Ok(())
    });
    sim.run().unwrap();
}

#[test]
fn filesystem_snapshot_and_chunk_survive_host_reopen() {
    use server::engine::fs_store::{FsStore, Manifest};
    use server::engine::heap_storage::SledHeapStorage;
    use std::sync::Arc;

    let mut sim = turmoil::Builder::new().build();
    sim.host("n0", || async {
        let config = || {
            sled::Config::new()
                .path("/snapshot-blobs")
                .flush_every_ms(None)
        };
        let db = config().open().unwrap();
        let store = FsStore::new(Arc::new(SledHeapStorage::new(db.clone()).unwrap()));
        let bytes = vec![0x42; 128 * 1024];
        let mut manifest = Manifest::default();
        manifest
            .entries
            .insert("data.bin".into(), store.put_file(&bytes).await.unwrap());
        let root = store.put_manifest(&manifest).await.unwrap();
        drop(store);
        drop(db);

        let reopened = config().open().unwrap();
        let store = FsStore::new(Arc::new(SledHeapStorage::new(reopened).unwrap()));
        let recovered = store.get_manifest(&root).await.unwrap();
        let entry = recovered
            .entries
            .get(std::path::Path::new("data.bin"))
            .unwrap();
        assert_eq!(store.read_file(entry).await.unwrap(), bytes);
        Ok(())
    });
    sim.run().unwrap();
}

#[test]
fn blocking_worker_blobs_are_flushed_by_host_and_reopen() {
    use server::engine::heap_storage::{HeapStorage, HostBlobStorage, SledHeapStorage};
    use std::sync::Arc;
    let mut sim = turmoil::Builder::new().build();
    sim.host("n0", || async {
        let config = || {
            sled::Config::new()
                .path("/worker-blobs")
                .flush_every_ms(None)
        };
        let db = config().open().unwrap();
        let storage = Arc::new(SledHeapStorage::new(db.clone()).unwrap());
        let (bridge, pump) = HostBlobStorage::bridge(storage);
        let pump = tokio::spawn(pump);
        let from_worker = tokio::task::spawn_blocking(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async move {
                bridge.put("payload", b"worker bytes").await.unwrap();
                bridge.get("payload").await.unwrap()
            })
        })
        .await
        .unwrap();
        assert_eq!(from_worker, b"worker bytes");
        pump.abort();
        let _ = pump.await;
        drop(db);
        let reopened = SledHeapStorage::new(config().open().unwrap()).unwrap();
        assert_eq!(reopened.get("payload").await.unwrap(), b"worker bytes");
        Ok(())
    });
    sim.run().unwrap();
}

#[test]
fn host_owned_engine_runs_js_on_patched_sled() {
    use engine_cluster_tests::scenario::engine_with_cluster_host;
    use server::cluster::{ClusterConfig, ClusterNode};
    use server::engine::parse_ca_hex;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    let done = Arc::new(AtomicBool::new(false));
    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(45))
        .build();
    let host_done = done.clone();
    sim.host("n0", move || {
        let done = host_done.clone();
        async move {
            let db = sled::Config::new()
                .path("/host-engine")
                .flush_every_ms(None)
                .open()
                .unwrap();
            let node = ClusterNode::new(
                ClusterConfig {
                    node_id: "n0".into(),
                    election_timeout_min: Duration::from_millis(50),
                    election_timeout_max: Duration::from_millis(50),
                    ..ClusterConfig::default()
                },
                db.clone(),
            );
            node.start_background_tasks().unwrap();
            tokio::time::sleep(Duration::from_millis(200)).await;
            let (engine, store, pump, console_pump, console_bridge) = engine_with_cluster_host(node, db);
            let program = Program(vec![Step::Write {
                path: "/data/iteration".into(),
                bytes: "host-owned".into(),
            }]);
            let id = engine
                .run_js(program.render_js(0))
                .maybe_fs(Some("main".into()))
                .execute()
                .await
                .unwrap();
            let mut head: Option<String> = None;
            for _ in 0..1200 {
                tokio::time::sleep(Duration::from_millis(25)).await;
                let info = engine.get_execution(id.clone()).unwrap();
                if info.status == "failed" {
                    panic!("host JS failed: {:?}", info.error);
                }
                if info.status == "completed" {
                    head = info.fs;
                    break;
                }
            }
            let head = head.expect("host JS did not publish a snapshot head");
            let entry = store
                .resolve_file(
                    Some(parse_ca_hex(&head).unwrap()),
                    &["data".into(), "iteration".into()],
                )
                .await
                .unwrap()
                .unwrap();
            assert_eq!(store.read_file(&entry).await.unwrap(), b"host-owned");
            console_bridge.revoke();
            console_pump.abort();
            let _ = console_pump.await;
            pump.abort();
            let _ = pump.await;
            done.store(true, Ordering::SeqCst);
            Ok(())
        }
    });
    sim.client("observer", async move {
        for _ in 0..2000 {
            if done.load(Ordering::SeqCst) {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Err("host JS did not finish".into())
    });
    sim.run().unwrap();
}

#[test]
fn observer_runs_js_and_publishes_label_on_host() {
    use engine_cluster_tests::scenario::{JsResult, ScenarioHost, ScenarioHostClient};
    use server::cluster::{ClusterConfig, ClusterNode, Role};
    use std::time::Duration;
    use turmoil::net::TcpListener;

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(45))
        .build();
    sim.host("n0", || async {
        let db = sled::Config::new()
            .path("/rpc-engine")
            .flush_every_ms(None)
            .open()
            .unwrap();
        let node = ClusterNode::new(
            ClusterConfig {
                node_id: "n0".into(),
                election_timeout_min: Duration::from_millis(50),
                election_timeout_max: Duration::from_millis(50),
                ..ClusterConfig::default()
            },
            db.clone(),
        );
        node.start_background_tasks().unwrap();
        let host = ScenarioHost::new(node, db.clone());
        let listener = TcpListener::bind(("0.0.0.0", 4301)).await?;
        for _ in 0..7 {
            let (stream, _) = listener.accept().await?;
            host.serve_request(stream).await.unwrap();
        }
        let output: Vec<u8> = db.tree_names().into_iter()
            .filter(|name| name.starts_with(b"ex:"))
            .flat_map(|name| db.open_tree(name).unwrap().iter()
                .flat_map(|entry| entry.unwrap().1.to_vec()))
            .collect();
        assert!(String::from_utf8_lossy(&output).contains("scoped console"),
            "host console output was lost: {output:?}");
        Ok(())
    });
    sim.client("observer", async {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let client = ScenarioHostClient::new("n0");
        assert_eq!(client.status().await.unwrap().0, Role::Leader);
        let code = format!(
            "console.log('scoped console');{}",
            Program(vec![Step::Write {
                path: "/data/iteration".into(),
                bytes: "rpc-owned".into(),
            }]).render_js(0),
        );
        let head = match client.run_js(&code).await.unwrap() {
            JsResult::Head(head) => head,
            other => panic!("host JS returned {other:?}"),
        };
        assert_eq!(
            client
                .read_file(&head, "/data/iteration")
                .await
                .unwrap()
                .unwrap(),
            b"rpc-owned"
        );
        let ca = server::engine::parse_ca_hex(&head).unwrap();
        assert_eq!(client.resolve("main").await.unwrap(), None);
        assert!(client.cas("main", None, ca).await.unwrap());
        assert_eq!(client.resolve("main").await.unwrap(), Some(ca));
        assert!(client.log("main").await.unwrap().iter().any(|entry| entry.to == ca));
        Ok(())
    });
    sim.run().unwrap();
}

#[test]
fn owner_two_crashes_keep_distinct_acknowledged_js_snapshots() {
    use engine_cluster_tests::scenario::{JsResult, ScenarioHost, ScenarioHostClient};
    use server::cluster::{ClusterConfig, ClusterNode, Role};
    use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};
    use std::time::Duration;
    use turmoil::net::TcpListener;

    let stage = Arc::new(AtomicUsize::new(0));
    let generation = Arc::new(AtomicUsize::new(0));
    let mut sim = turmoil::Builder::new().simulation_duration(Duration::from_secs(80)).build();
    let opened = generation.clone();
    sim.host("n0", move || {
        let opened = opened.clone();
        async move {
            let db = sled::Config::new().path("/crash-js").flush_every_ms(None).open().unwrap();
            let incarnation = opened.fetch_add(1, Ordering::SeqCst) + 1;
            let node = ClusterNode::new(ClusterConfig {
                node_id: "n0".into(),
                election_timeout_min: Duration::from_millis(50),
                election_timeout_max: Duration::from_millis(50),
                ..ClusterConfig::default()
            }, db.clone());
            node.start_background_tasks().unwrap();
            let host = ScenarioHost::new_with_generation(node, db, incarnation as u64);
            let listener = TcpListener::bind(("0.0.0.0", 4301)).await?;
            loop {
                let (stream, _) = listener.accept().await?;
                host.serve_request(stream).await.unwrap();
            }
        }
    });
    let signal = stage.clone();
    sim.client("observer", async move {
        let client = ScenarioHostClient::new("n0");
        for _ in 0..100 {
            if matches!(client.status().await, Ok((Role::Leader, _))) { break; }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(matches!(client.status().await, Ok((Role::Leader, _))));
        assert_eq!(client.generation().await.unwrap(), 1);
        let old_code = Program(vec![Step::Write {
            path: "/data/iteration".into(), bytes: "before-crash".into(),
        }]).render_js(0);
        let old_head = match client.run_js(&old_code).await.unwrap() {
            JsResult::Head(head) => head,
            other => panic!("initial JS: {other:?}"),
        };
        let old_ca = server::engine::parse_ca_hex(&old_head).unwrap();
        assert!(client.cas("main", None, old_ca).await.unwrap());
        assert_eq!(client.read_file(&old_head, "/data/iteration").await.unwrap(), Some(b"before-crash".to_vec()));
        signal.store(1, Ordering::SeqCst);
        while signal.load(Ordering::SeqCst) < 2 { tokio::time::sleep(Duration::from_millis(20)).await; }
        for _ in 0..100 {
            if matches!(client.status().await, Ok((Role::Leader, _))) { break; }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(client.generation().await.unwrap(), 2);
        assert_eq!(client.resolve("main").await.unwrap(), Some(old_ca));
        assert_eq!(client.read_file(&old_head, "/data/iteration").await.unwrap(), Some(b"before-crash".to_vec()));
        let new_code = Program(vec![Step::Write {
            path: "/data/iteration".into(), bytes: "after-recovery".into(),
        }]).render_js(1);
        let new_head = match client.run_js(&new_code).await.unwrap() {
            JsResult::Head(head) => head,
            other => panic!("recovery JS: {other:?}"),
        };
        let new_ca = server::engine::parse_ca_hex(&new_head).unwrap();
        assert_ne!(new_ca, old_ca, "post-recovery head must advance");
        assert!(client.cas("main", Some(old_ca), new_ca).await.unwrap());
        signal.store(3, Ordering::SeqCst);
        while signal.load(Ordering::SeqCst) < 4 { tokio::time::sleep(Duration::from_millis(20)).await; }
        for _ in 0..100 {
            if matches!(client.status().await, Ok((Role::Leader, _))) { break; }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(client.generation().await.unwrap(), 3);
        assert_eq!(client.resolve("main").await.unwrap(), Some(new_ca));
        assert_eq!(client.read_file(&new_head, "/data/iteration").await.unwrap(), Some(b"after-recovery".to_vec()));
        signal.store(5, Ordering::SeqCst);
        Ok(())
    });
    for target in [1, 3, 5] {
        for _ in 0..50_000 {
            sim.step().unwrap();
            if stage.load(Ordering::SeqCst) == target { break; }
        }
        assert_eq!(stage.load(Ordering::SeqCst), target, "phase {target} did not finish");
        if target < 5 {
            sim.crash("n0");
            stage.store(target + 1, Ordering::SeqCst);
            sim.bounce("n0");
        }
    }
    assert_eq!(generation.load(Ordering::SeqCst), 3, "host did not reopen twice");
}

#[test]
fn invalid_snapshot_head_is_not_reported_as_absent_file() {
    use engine_cluster_tests::scenario::{ScenarioHost, ScenarioHostClient};
    use server::cluster::{ClusterConfig, ClusterNode};
    use std::time::Duration;
    use turmoil::net::TcpListener;

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(5))
        .build();
    sim.host("n0", || async {
        let db = sled::Config::new()
            .path("/invalid-head-rpc")
            .flush_every_ms(None)
            .open()
            .unwrap();
        let node = ClusterNode::new(
            ClusterConfig { node_id: "n0".into(), ..ClusterConfig::default() },
            db.clone(),
        );
        let host = ScenarioHost::new(node, db);
        let listener = TcpListener::bind(("0.0.0.0", 4301)).await?;
        let (stream, _) = listener.accept().await?;
        host.serve_request(stream).await.unwrap();
        Ok(())
    });
    sim.client("observer", async {
        tokio::time::sleep(Duration::from_millis(10)).await;
        let error = ScenarioHostClient::new("n0")
            .read_file("not-a-ca-id", "/data/iteration")
            .await
            .expect_err("malformed head must not be treated as an absent file");
        assert!(error.contains("invalid"), "{error}");
        Ok(())
    });
    sim.run().unwrap();
}

#[test]
fn post_recovery_js_write_cannot_replace_missing_precrash_ack() {
    let result = Scenario {
        seed: 42,
        topology: Topology {
            voters: ["n0", "n1", "n2"].map(str::to_owned).to_vec(),
            clients: vec!["observer".into()],
        },
        workload: Workload {
            every_ms: 250,
            count: 0,
            op: Op::RunJs {
                node: "leader".into(),
                program: Program(vec![Step::Write {
                    path: "/data/iteration".into(),
                    bytes: "post-only-{i}".into(),
                }]),
            },
        },
        faults: vec![Fault::Crash {
            at_ms: 900,
            node: CrashTarget::CurrentLeader,
        }],
        recovery: Recovery {
            stop_workload: true,
            heal_all: true,
            restart_crashed: true,
            deadline_ms: 10_000,
        },
        asserts: vec![Assertion::FreshWriteSucceeds],
    }
    .run_and_dump("real-sled-no-precrash-ack");
    assert_eq!(result.acked.len(), 1, "post-recovery publication: {:?}", result.events);
    assert!(matches!(&result.status, Err(error) if error.contains("FreshWriteSucceeds")),
        "a fresh write is not recovery evidence without a pre-crash ACK: status={:?} events={:?}", result.status, result.events);
}

#[test]
fn unavailable_replicas_do_not_count_as_converged() {
    use engine_cluster_tests::scenario::ScenarioEvent;
    let result = Scenario {
        seed: 42,
        topology: Topology {
            voters: ["n0", "n1", "n2"].map(str::to_owned).to_vec(),
            clients: vec!["observer".into()],
        },
        workload: Workload {
            every_ms: 250,
            count: 0,
            op: Op::RunJs {
                node: "leader".into(),
                program: Program(vec![Step::Write {
                    path: "/data/iteration".into(),
                    bytes: "never-published".into(),
                }]),
            },
        },
        faults: ["n0", "n1", "n2"].into_iter().enumerate()
            .map(|(i, name)| Fault::Crash {
                at_ms: 900 + i as u64 * 100,
                node: CrashTarget::Node(name.into()),
            }).collect(),
        recovery: Recovery {
            stop_workload: true,
            heal_all: true,
            restart_crashed: false,
            deadline_ms: 300,
        },
        asserts: vec![Assertion::ReplicasConvergeAfterRecovery],
    }
    .run_and_dump("real-sled-unavailable-replicas");
    assert!(result.events.iter().any(|event| matches!(event,
        ScenarioEvent::Assert { name, passed: false, .. }
            if name == "ReplicasConvergeAfterRecovery"
    )), "unavailable replicas falsely converged: status={:?} events={:?}",
        result.status, result.events);
}

#[test]
fn scheduled_crash_interrupts_in_flight_js_publication() {
    use engine_cluster_tests::scenario::ScenarioEvent;
    let result = Scenario {
        seed: 42,
        topology: Topology {
            voters: ["n0", "n1", "n2"].map(str::to_owned).to_vec(),
            clients: vec!["observer".into()],
        },
        workload: Workload {
            every_ms: 250,
            count: 1,
            op: Op::RunJs {
                node: "leader".into(),
                program: Program(vec![Step::Write {
                    path: "/data/iteration".into(),
                    bytes: "in-flight-{i}".into(),
                }]),
            },
        },
        faults: vec![Fault::Crash {
            at_ms: 640,
            node: CrashTarget::CurrentLeader,
        }],
        recovery: Recovery {
            stop_workload: true,
            heal_all: true,
            restart_crashed: true,
            deadline_ms: 10_000,
        },
        asserts: vec![],
    }
    .run_and_dump("real-sled-inflight-crash");
    let invoke = result.events.iter().position(|event| matches!(event, ScenarioEvent::Invoke { .. }))
        .expect("JS operation never started");
    let crash = result.events.iter().position(|event| matches!(event, ScenarioEvent::Crash { .. }))
        .expect("scheduled owner crash never occurred");
    let respond = result.events.iter().position(|event| matches!(event, ScenarioEvent::Respond { .. }))
        .expect("in-flight JS operation never responded");
    assert!(invoke < crash && crash < respond,
        "fault missed in-flight publication: status={:?} events={:?}", result.status, result.events);
}

#[test]
fn dropping_scenario_host_stops_its_blob_pump() {
    use engine_cluster_tests::scenario::ScenarioHost;
    use server::cluster::{ClusterConfig, ClusterNode};
    use std::time::Duration;

    let mut sim = turmoil::Builder::new()
        .simulation_duration(Duration::from_secs(2))
        .build();
    sim.host("n0", || async {
        let db = sled::Config::new()
            .path("/pump-drop")
            .flush_every_ms(None)
            .open()
            .unwrap();
        let node = ClusterNode::new(
            ClusterConfig { node_id: "n0".into(), ..ClusterConfig::default() },
            db.clone(),
        );
        let host = ScenarioHost::new(node, db);
        let pump = host.pump_abort_handle();
        assert!(!pump.is_finished(), "pump should run while host is alive");
        drop(host);
        for _ in 0..10 {
            if pump.is_finished() { break; }
            tokio::task::yield_now().await;
        }
        assert!(pump.is_finished(), "dropping the host detached its sled-backed blob pump");
        Ok(())
    });
    sim.run().unwrap();
}

#[test]
fn stale_console_writer_is_fenced_after_fresh_reopen() {
    use engine_cluster_tests::scenario::{ScenarioHost, ScenarioHostClient};
    use server::cluster::{ClusterConfig, ClusterNode, Role};
    use server::engine::console::HostConsoleBridge;
    use std::sync::{Arc, atomic::{AtomicUsize, Ordering}, OnceLock};
    use std::time::Duration;
    use turmoil::net::TcpListener;

    // NOTE: a genuinely in-flight worker cannot be crashed mid-JS under
    // turmoil 0.7.2 + this engine: the host runtime's simulated clock is
    // frozen while the awaited spawn_blocking worker runs (probed via CI:
    // the watcher never observes mid-flight; Sim::crash only runs between
    // steps). The worker therefore always completes before the crash, and
    // its console output is legitimately durable. What MUST hold — and what
    // this regression now checks deterministically — is that a console
    // writer captured before the crash is fenced against the reopened
    // generation, and that completed pre-crash output survives.
    let stage = Arc::new(AtomicUsize::new(0));
    let generation = Arc::new(AtomicUsize::new(0));
    let bridge_slot: Arc<OnceLock<Arc<HostConsoleBridge>>> = Arc::new(OnceLock::new());
    let mut sim = turmoil::Builder::new().simulation_duration(Duration::from_secs(30)).build();
    let host_stage = stage.clone();
    let opened = generation.clone();
    let host_bridge = bridge_slot.clone();
    sim.host("n0", move || {
        let stage = host_stage.clone();
        let opened = opened.clone();
        let host_bridge = host_bridge.clone();
        async move {
            let db = sled::Config::new().path("/stale-console").flush_every_ms(None).open().unwrap();
            let incarnation = opened.fetch_add(1, Ordering::SeqCst) + 1;
            let node = ClusterNode::new(ClusterConfig {
                node_id: "n0".into(),
                election_timeout_min: Duration::from_millis(50),
                election_timeout_max: Duration::from_millis(50),
                ..ClusterConfig::default()
            }, db.clone());
            node.start_background_tasks().unwrap();
            let host = ScenarioHost::new_with_generation(node, db.clone(), incarnation as u64);
            if incarnation == 1 {
                let _ = host_bridge.set(host.console_bridge());
                stage.store(1, Ordering::SeqCst);
            } else {
                let mut waits = 0usize;
                while stage.load(Ordering::SeqCst) < 2 {
                    waits += 1;
                    if waits == 1 || waits == 100 || waits == 1000 {
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                let output: Vec<u8> = db.tree_names().into_iter()
                    .filter(|name| name.starts_with(b"ex:"))
                    .flat_map(|name| db.open_tree(name).unwrap().iter()
                        .flat_map(|entry| entry.unwrap().1.to_vec()))
                    .collect();
                assert!(String::from_utf8_lossy(&output).contains("worker-entered"),
                    "pre-crash console output must survive the fresh reopen");
                assert!(!String::from_utf8_lossy(&output).contains("fenced-stale-probe"),
                    "a post-crash stale writer must not reach the reopened generation");
                stage.store(3, Ordering::SeqCst);
            }
            let listener = TcpListener::bind(("0.0.0.0", 4301)).await?;
            loop {
                let (stream, _) = listener.accept().await?;
                let _ = host.serve_request(stream).await;
            }
        }
    });
    let client_stage = stage.clone();
    sim.client("observer", async move {
        let client = ScenarioHostClient::new("n0");
        for _ in 0..100 {
            if matches!(client.status().await, Ok((Role::Leader, _))) { break; }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let code = "console.log('worker-entered' + 'x'.repeat(5000)); console.log('done-marker' + 'y'.repeat(5000));";
        let result = client.run_js(code).await;
        client_stage.store(2, Ordering::SeqCst);
        Ok(())
    });
    for _ in 0..50_000 {
        sim.step().unwrap();
        if stage.load(Ordering::SeqCst) == 2 { break; }
    }
    assert_eq!(stage.load(Ordering::SeqCst), 2, "execution never completed before the crash");
    let writer = bridge_slot.get().expect("console bridge not published")
        .for_execution("fence-probe".to_string());
    sim.crash("n0");
    sim.bounce("n0");
    for _ in 0..50_000 {
        sim.step().unwrap();
        if generation.load(Ordering::SeqCst) == 2 { break; }
    }
    assert_eq!(generation.load(Ordering::SeqCst), 2, "fresh DB did not reopen");
    // NOTE: no stage.store here — the client already set stage=2 (JS done),
    // and the reopened host may have already advanced it to 3; overwriting
    // would clobber that progress.
    for _ in 0..50_000 {
        sim.step().unwrap();
        if stage.load(Ordering::SeqCst) == 3 { break; }
    }
    assert_eq!(stage.load(Ordering::SeqCst), 3, "reopened host did not check console");
    let fenced = writer.write_page(0, b"fenced-stale-probe".to_vec());
    assert!(fenced.is_err(),
        "a console writer captured before the crash must be fenced after the reopen: {fenced:?}");
}
