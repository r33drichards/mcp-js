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
