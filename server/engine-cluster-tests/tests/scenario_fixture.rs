//! Milestone 1+2 of issue #316: a declarative scenario, one runner, an
//! event log, replay, and a known-answer fixture exercising a real JS
//! write/publish, an interrupted second publish whose outcome is unknown,
//! recovery, and reconciliation of the unknown operation by observation.
//!
//! The scenario is data: the same JSON round-trips through the runner, so a
//! failing input can be replayed without fuzzing.
use engine_cluster_tests::scenario::{Assertion, Fault, Op, Program, Scenario, Step};

const KNOWN_ANSWER: &str = r#"{
  "seed": 42,
  "topology": { "voters": ["n0", "n1", "n2"], "clients": ["observer"] },
  "workload": { "every_ms": 250, "count": 3, "op": { "run_js": { "node": "leader", "program": [
    { "write": { "path": "/data/iteration", "bytes": "iteration-{i}" } }
  ] } } },
  "faults": [
    { "partition": { "at_ms": 2000, "groups": [["n0"], ["n1", "n2"]] } },
    { "heal_all": { "at_ms": 4000 } }
  ],
  "recovery": { "stop_workload": true, "heal_all": true, "restart_crashed": true, "deadline_ms": 10000 },
  "asserts": [
    "acked_commits_in_legal_history",
    "replicas_converge_after_recovery",
    "published_snapshot_matches_model"
  ]
}"#;

fn parse_known_answer() -> Scenario {
    Scenario::from_json(KNOWN_ANSWER).expect("known-answer scenario parses")
}

#[test]
fn scenario_round_trips_through_json() {
    let scenario = parse_known_answer();
    let json = scenario.to_json_pretty();
    let reparsed = Scenario::from_json(&json).expect("reparse");
    assert_eq!(scenario, reparsed, "scenario must round-trip losslessly");
    assert_eq!(scenario.seed, 42);
    assert_eq!(scenario.topology.voters, vec!["n0", "n1", "n2"]);
    assert_eq!(scenario.workload.count, 3);
    assert_eq!(scenario.faults.len(), 2);
    assert_eq!(
        scenario.asserts,
        vec![
            Assertion::AckedCommitsInLegalHistory,
            Assertion::ReplicasConvergeAfterRecovery,
            Assertion::PublishedSnapshotMatchesModel,
        ]
    );
}

#[test]
fn scenario_rejects_unknown_fields_and_bad_topologies() {
    assert!(Scenario::from_json(r#"{"seed": 1, "bogus": true}"#).is_err());
    // Bounded topology: exactly three voters and one client are supported.
    let mut scenario = parse_known_answer();
    scenario.topology.voters = vec!["n0".to_string(), "n1".to_string()];
    assert!(
        Scenario::from_json(&scenario.to_json()).is_err(),
        "two-voter topology must be rejected, not silently run"
    );
}

#[test]
fn known_answer_scenario_replays_and_passes() {
    let scenario = parse_known_answer();
    // The runner persists the scenario, event log, resolved hosts, and results
    // as artifacts, so a failure here replays without fuzzing.
    let result = scenario.run_and_dump("known-answer-001");
    result.expect_status("all assertions passed");
    // Every acknowledged commit stayed in the legal history: the final head is
    // either the last acked commit or the head of an unresolved (unknown)
    // final operation, never an unobserved value.
    let history = result.legal_history();
    assert!(!history.acked.is_empty(), "fixture must ack at least one commit");
    let heads = result.final_heads();
    assert_eq!(
        heads.len(),
        3,
        "all three replicas must expose a head: {heads:?}"
    );
    let converged: Vec<_> = heads.values().collect();
    assert!(
        converged.windows(2).all(|w| w[0] == w[1]),
        "replicas must converge after recovery: {heads:?}"
    );
    // The last acked iteration's content matches the independent model.
    let model = result.model_snapshot();
    assert!(
        model.iter().any(|(path, bytes)| path == "/data/iteration"
            && bytes == b"iteration-2"),
        "model must hold the last acknowledged iteration, got {model:?}"
    );
}

#[test]
fn interrupted_publish_is_unknown_not_failed() {
    // Milestone 2's interrupted second publish: full partition (every host
    // alone, no quorum anywhere) after the first publish so the second push
    // can reach no quorum and cannot be served by a re-elected leader. The
    // runner must classify it as unknown, not failed, and reconcile it only
    // by observation.
    let mut scenario = parse_known_answer();
    scenario.workload.count = 2;
    scenario.workload.every_ms = 1000;
    scenario.faults = vec![
        Fault::Partition {
            at_ms: 900,
            groups: vec![vec!["current_leader".into()], vec!["n1".into()], vec!["n2".into()], vec!["observer".into()]],
        },
        Fault::HealAll { at_ms: 5000 },
    ];
    let result = scenario.run_and_dump("known-answer-002");
    let outcome = result.last_outcome();
    match outcome {
        // A partitioned/re-elected serving node answers definitively
        // (conflict); a truly lost response is unknown. Either way the
        // interrupted publish must not report success.
        Some(("unknown", _)) | Some(("conflict", _)) => {}
        other => panic!("interrupted publish must not report success, got {other:?}"),
    }
    assert_eq!(
        result.acked.len(),
        1,
        "only the pre-partition publish may ack: {:?}",
        result.acked
    );
    result.expect_status("all assertions passed");
}

#[test]
fn replay_command_reruns_a_persisted_scenario() {
    let scenario = parse_known_answer();
    let json = scenario.to_json();
    // Replay: the persisted artifact re-enters the runner directly, no fuzzing.
    let replayed = Scenario::from_json(&json).expect("replay parse");
    let result = replayed.run_and_dump("known-answer-replay");
    result.expect_status("all assertions passed");
}

#[test]
fn program_renders_to_real_js_with_iteration_variable() {
    let program = Program(vec![Step::Write {
        path: "/data/iteration".into(),
        bytes: "iteration-{i}".into(),
    }]);
    let js = program.render_js(2);
    assert!(
        js.contains("iteration-2"),
        "rendered JS must substitute the iteration index: {js}"
    );
    assert!(
        js.contains("fs.writeFile"),
        "rendered JS must go through the real fs surface: {js}"
    );
    // Dynamic selectors resolve at a defined boundary and are recorded.
    assert_eq!(Op::RunJs { node: "leader".into(), program }.resolve_node("n1"), "n1");
}

#[test]
fn unmodeled_js_classifies_exception_without_claiming_exact_output() {
    let mut scenario = parse_known_answer();
    scenario.workload.count = 1;
    scenario.workload.op = Op::RunJsUnmodeled {
        node: "leader".into(),
        source: "(async () => { try { throw new TypeError('expected'); } catch (e) { return '__mcp_js_expected_exception__' + e.name; } })()".into(),
    };
    scenario.faults.clear();
    scenario.asserts = vec![
        Assertion::NoUnexpectedJsFailure,
        Assertion::AckedCommitsInLegalHistory,
        Assertion::ReplicasConvergeAfterRecovery,
    ];
    let parsed = Scenario::from_json(&scenario.to_json()).expect("unmodeled replay parses");
    let result = parsed.run_and_dump("unmodeled-exception");
    result.expect_status("all assertions passed");
    assert_eq!(result.last_outcome().map(|(kind, _)| kind), Some("js_exception"));
}

#[test]
fn unmodeled_js_rejects_unbounded_source_and_exact_model_assertion() {
    let mut scenario = parse_known_answer();
    scenario.workload.op = Op::RunJsUnmodeled { node: "leader".into(), source: "x".repeat(513) };
    assert!(Scenario::from_json(&scenario.to_json()).is_err());
    scenario.workload.op = Op::RunJsUnmodeled { node: "leader".into(), source: "1 + 1".into() };
    assert!(Scenario::from_json(&scenario.to_json()).is_err(), "no false exact-output model");
    scenario.asserts = vec![Assertion::NoUnexpectedJsFailure];
    assert!(Scenario::from_json(&scenario.to_json()).is_err(), "execution count must be bounded");
    scenario.workload.count = 0;
    assert!(Scenario::from_json(&scenario.to_json()).is_err(), "zero executions cannot pass vacuously");
    scenario.workload.count = 1;
    assert!(Scenario::from_json(&scenario.to_json()).is_ok());
}

#[test]
fn unmodeled_uncaught_error_is_not_misclassified_by_its_text() {
    let mut scenario = parse_known_answer();
    scenario.workload.count = 1;
    scenario.workload.op = Op::RunJsUnmodeled {
        node: "leader".into(),
        source: "(async () => { throw new TypeError('not tagged'); })()".into(),
    };
    scenario.faults.clear();
    scenario.asserts = vec![Assertion::NoUnexpectedJsFailure];
    let result = scenario.run_and_dump("unmodeled-untagged-error");
    assert!(result.status.is_err(), "untagged failures must not pass the oracle");
    assert_eq!(result.last_outcome().map(|(kind, _)| kind), Some("failed"));
}
