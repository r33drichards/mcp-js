//! Opt-in integration: the same JS scenario runner linked against Turmoil-backed sled.
#![cfg(feature = "scenario")]

use engine_cluster_tests::scenario::{Assertion, CrashTarget, Fault, Op, Program, Recovery, Scenario, Step, Topology, Workload};

#[test]
fn real_sled_reopens_after_crash_and_acknowledges_new_js_write() {
    let scenario = Scenario {
        seed: 42,
        topology: Topology { voters: ["n0", "n1", "n2"].map(str::to_owned).to_vec(), clients: vec!["observer".into()] },
        workload: Workload {
            every_ms: 250,
            count: 1,
            op: Op::RunJs {
                node: "leader".into(),
                program: Program(vec![Step::Write { path: "/data/iteration".into(), bytes: "iteration-{i}".into() }]),
            },
        },
        faults: vec![Fault::Crash { at_ms: 900, node: CrashTarget::Node("n1".into()) }],
        recovery: Recovery { stop_workload: true, heal_all: true, restart_crashed: true, deadline_ms: 10_000 },
        asserts: vec![
            Assertion::FreshWriteSucceeds,
            Assertion::AckedCommitsInLegalHistory,
            Assertion::ReplicasConvergeAfterRecovery,
            Assertion::PublishedSnapshotMatchesModel,
        ],
    };
    let result = scenario.run_and_dump("real-sled-crash-js");
    result.expect_status("all assertions passed");
    assert_eq!(result.acked.len(), 2, "old and fresh JS writes need acknowledgments: {:?}", result.events);
}
