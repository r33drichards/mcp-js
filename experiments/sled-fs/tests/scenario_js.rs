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
            node: CrashTarget::Node("n1".into()),
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
}

#[test]
fn execution_registry_reuses_host_owned_persistent_sled() {
    use server::engine::execution::ExecutionRegistry;
    let mut sim = turmoil::Builder::new().build();
    sim.host("n0", || async {
        let config = || sled::Config::new().path("/registry").flush_every_ms(None);
        let db = config().open().unwrap();
        let registry = ExecutionRegistry::from_db(db.clone());
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
            let (engine, store, pump) = engine_with_cluster_host(node, db);
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
            let mut head = None;
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
