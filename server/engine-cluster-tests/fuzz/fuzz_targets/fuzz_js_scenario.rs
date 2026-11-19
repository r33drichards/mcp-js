//! Milestone 3 of issue #316: a coverage harness that maps arbitrary bytes to
//! a bounded structured scenario and executes it with the SAME declarative
//! runner as the fixture tests. Every scenario is serializable, so a failing
//! input reproduces as a scenario.json artifact without the fuzzer.
//!
//! Bounded by construction: fixed three-voter topology, at most two workload
//! iterations, at most two faults, and bounded fault windows, so one exec
//! stays near the CI runner's budget.
#![no_main]
use engine_cluster_tests::scenario::{Assertion, CrashTarget, Fault, Op, Program, Recovery, Scenario, Step, Topology, Workload};
use libfuzzer_sys::fuzz_target;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545F4914F6CDD1D)
    }
    fn below(&mut self, n: u64) -> u64 { self.next() % n }
}

fuzz_target!(|data: &[u8]| {
    if data.len() < 8 {
        return;
    }
    let mut r = Rng(u64::from_le_bytes(data[0..8].try_into().unwrap()) | 1);
    let seed = r.next();

    // 1-2 JS write iterations on the resolved leader.
    let count = 1 + r.below(2) as usize;
    let every_ms = 150 + r.below(3) * 50;
    let mut steps = Vec::new();
    let step_count = 1 + r.below(2) as usize;
    for s in 0..step_count {
        let payload: String = data
            .iter()
            .skip(8)
            .take(8)
            .map(|b| {
                if b.is_ascii_alphanumeric() {
                    *b as char
                } else {
                    'x'
                }
            })
            .chain(std::iter::once((b'0' + (s as u8)) as char))
            .collect();
        steps.push(Step::Write {
            path: format!("/data/fuzz-{}", s),
            bytes: payload,
        });
    }

    // 0-2 faults drawn from partition shapes, heal, and leader crash.
    let mut faults: Vec<Fault> = Vec::new();
    let fault_count = r.below(3);
    let mut at = 500 + r.below(400);
    for _ in 0..fault_count {
        let kind = r.below(3);
        let fault = match kind {
            0 => Fault::Partition {
                at_ms: at,
                groups: match r.below(3) {
                    0 => vec![vec!["n0".into()], vec!["n1".into(), "n2".into()]],
                    1 => vec![vec!["n0".into()], vec!["n1".into()], vec!["n2".into()]],
                    _ => vec![vec!["n0".into(), "n1".into()], vec!["n2".into()]],
                },
            },
            1 => Fault::HealAll { at_ms: at },
            _ => Fault::Crash {
                at_ms: at,
                node: CrashTarget::CurrentLeader,
            },
        };
        faults.push(fault);
        at += 800 + r.below(800);
    }

    let scenario = Scenario {
        seed,
        topology: Topology {
            voters: vec!["n0".into(), "n1".into(), "n2".into()],
            clients: vec!["observer".into()],
        },
        workload: Workload {
            every_ms,
            count,
            op: Op::RunJs {
                node: "leader".into(),
                program: Program(steps),
            },
        },
        faults,
        recovery: Recovery {
            stop_workload: true,
            heal_all: true,
            restart_crashed: true,
            deadline_ms: 6000,
        },
        asserts: vec![
            Assertion::AckedCommitsInLegalHistory,
            Assertion::ReplicasConvergeAfterRecovery,
            Assertion::PublishedSnapshotMatchesModel,
            Assertion::FreshWriteSucceeds,
        ],
    };

    // Serialize -> parse: the same path a saved artifact replays through.
    let json = scenario.to_json();
    let parsed = Scenario::from_json(&json).expect("generated scenario parses");
    let _result = parsed.run_and_dump("fuzz-scenario"); // assertions run inside the runner
});
