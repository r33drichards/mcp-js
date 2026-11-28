//! Milestone 4 of #316: bounded, broader JavaScript on a real cluster node.
//! This intentionally has no exact-output filesystem oracle: exceptions and
//! timeouts are classified, while engine failures, illegal acknowledged heads,
//! and failure to converge after healing are bugs. The serialized source and
//! fault schedule replay through the same scenario runner as fixtures.
#![no_main]
use engine_cluster_tests::scenario::{
    Assertion, Fault, Op, Recovery, Scenario, Topology, Workload,
};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() < 8 {
        return;
    }
    let seed = u64::from_le_bytes(data[..8].try_into().unwrap());
    let payload = data.get(8..).unwrap_or_default();
    let quoted = serde_json::to_string(&String::from_utf8_lossy(&payload[..payload.len().min(12)]))
        .expect("JSON string");
    // Mutate both valid programs and parser/exception paths. All source is
    // bounded by the scenario validator; no arbitrary result is assumed.
    let fs_statement = if payload.first().copied().unwrap_or(0) & 1 != 0 {
        format!("await fs.writeFile('/data/unmodeled', {quoted});")
    } else {
        String::new()
    };
    let mut statements = Vec::new();
    for (i, byte) in payload.iter().take(3).enumerate() {
        statements.push(match byte % 7 {
            0 => format!("const a{i} = JSON.parse({quoted});"),
            1 => format!("const a{i} = [1, 2, {byte}].filter(x => x % 2);"),
            2 => format!("const b{i} = new Uint8Array([{byte}, {i}]).map(x => x ^ 7);"),
            3 => format!(
                "const c{i} = await Promise.all([Promise.resolve({byte}), Promise.resolve({i})]);"
            ),
            4 => "throw new TypeError('expected fuzz exception');".into(),
            5 => format!("const d{i} = Object.fromEntries([['k', {quoted}]]);"),
            _ => {
                // Parser mutation over expression punctuation only: no
                // identifiers, calls, loops, or console writes can be formed.
                const TOKENS: &[u8] = b"01+-*/()[]{},.:";
                let raw: String = payload
                    .iter()
                    .skip(i)
                    .take(12)
                    .map(|b| TOKENS[*b as usize % TOKENS.len()] as char)
                    .collect();
                format!(
                    "const parsed{i} = eval({});",
                    serde_json::to_string(&raw).unwrap()
                )
            }
        });
    }
    if statements.is_empty() {
        statements.push("const x = 1 + 2;".into());
    }
    // Filesystem/engine failures are outside the catch; only ordinary JS
    // exceptions from the bounded pure-JS body carry the explicit marker.
    let source = format!(
        "export const __mcp_http_result = await (async () => {{ {fs_statement} try {{ {} }} catch (e) {{ return '__mcp_js_expected_exception__' + e.name; }} }})();",
        statements.join("\n")
    );
    // The bounded generator can exceed the validator when JSON escaping expands
    // control bytes; skip that candidate rather than treating it as a crash.
    if source.len() > 512 { return; }
    let mut faults = Vec::new();
    if payload.get(3).copied().unwrap_or(0) & 1 != 0 {
        faults.push(Fault::Partition {
            at_ms: 700,
            groups: vec![
                vec!["n0".into()],
                vec!["n1".into(), "n2".into()],
                vec!["observer".into()],
            ],
        });
        faults.push(Fault::HealAll { at_ms: 900 });
    }
    let scenario = Scenario {
        seed,
        topology: Topology {
            voters: vec!["n0".into(), "n1".into(), "n2".into()],
            clients: vec!["observer".into()],
        },
        workload: Workload {
            every_ms: 150,
            count: 1,
            op: Op::RunJsUnmodeled {
                node: "leader".into(),
                source,
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
            Assertion::NoUnexpectedJsFailure,
            Assertion::AckedCommitsInLegalHistory,
            Assertion::ReplicasConvergeAfterRecovery,
        ],
    };
    let replay = Scenario::from_json(&scenario.to_json()).expect("generated scenario parses");
    replay.assert_fuzz_case(&format!("fuzz-js-unmodeled-{}-{seed}", std::process::id()));
});
