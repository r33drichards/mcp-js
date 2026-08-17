//! Node.js core test harness.
//!
//! Runs the vendored subset of nodejs/node test/parallel files (see
//! tools/compat/vendor-node-tests.sh) inside the real engine via
//! `execute_stateless`, against the `node:` compat modules served by the
//! module loader. Each file runs in a fresh isolate: an ESM prelude
//! provides the CJS shell (require over the node: registry, the
//! `../common` module with mustCall tracking, process/Buffer globals),
//! then the test body is evaluated with classic-script semantics and a
//! drain-time reporter prints a JSON result under a sentinel.
//!
//! Results are locked against tests/node_compat/expectations.json with the
//! same drift model as the WPT harness: any change — regression OR
//! improvement — fails until re-recorded with NODE_COMPAT_UPDATE=1.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use server::engine::ExecutionConfig;
use server::engine::fetch::FetchConfig;
use server::engine::opa::{EvalMode, PolicyChain};

const PRELUDE_JS: &str = include_str!("node_compat/runner/prelude.js");
const RESULT_SENTINEL: &str = "__NODE_TEST_RESULT__";
const PER_FILE_TIMEOUT: Duration = Duration::from_secs(60);

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/node_compat")
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
enum Expectation {
    Pass(bool),
    Detail(ExpectationDetail),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
struct ExpectationDetail {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    ignore: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

type Expectations = BTreeMap<String, Expectation>;

#[derive(Debug, Deserialize)]
struct TestReport {
    skipped: Option<String>,
    failures: Vec<String>,
}

static INIT: std::sync::Once = std::sync::Once::new();

fn ensure_v8() {
    INIT.call_once(server::engine::initialize_v8);
}

fn collect_tests(vendor: &Path) -> Vec<PathBuf> {
    let dir = vendor.join("test/parallel");
    let mut out: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "js"))
        .collect();
    out.sort();
    out
}

fn assemble(test_path: &Path) -> Result<String, String> {
    let body = std::fs::read_to_string(test_path)
        .map_err(|e| format!("read {}: {e}", test_path.display()))?;
    let name = test_path.file_name().unwrap().to_string_lossy();
    let mut source = String::new();
    source.push_str(&format!(
        "globalThis.__NODE_TEST_NAME__ = {};\n",
        serde_json::to_string(name.as_ref()).unwrap()
    ));
    source.push_str(PRELUDE_JS);
    // Classic-script semantics for the CJS test body (top-level function/
    // var declarations become globals), same as the WPT harness.
    source.push_str(&format!(
        "try {{\n  (0, eval)({});\n}} catch (e) {{\n\
         \x20 if (!(e && e.__nodeTestSkip)) throw e;\n}}\n",
        serde_json::to_string(&body).unwrap()
    ));
    // Report after timers drain; 50ms keeps the loop alive past the
    // zero-delay timers the event tests use.
    source.push_str(&format!(
        "setTimeout(() => {{ console.log({:?} + globalThis.__NODE_TEST_REPORT__()); }}, 50);\n",
        RESULT_SENTINEL
    ));
    Ok(source)
}

enum Outcome {
    Pass,
    Fail(String),
}

fn run_file(test_path: &Path) -> Outcome {
    let source = match assemble(test_path) {
        Ok(s) => s,
        Err(e) => return Outcome::Fail(e),
    };

    let tmp = std::env::temp_dir().join(format!(
        "mcp-node-compat-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let db = sled::open(&tmp).expect("open sled db");
    let tree = db.open_tree("console").expect("open tree");

    let fetch_config = FetchConfig::new_with_chain(Arc::new(PolicyChain::new(
        vec![],
        EvalMode::All,
    )));
    let config = ExecutionConfig::new(256 * 1024 * 1024)
        .console_tree(tree.clone())
        .fetch_config(&fetch_config);
    let isolate_handle = config.isolate_handle.clone();

    let done = Arc::new(AtomicBool::new(false));
    let watchdog = {
        let done = done.clone();
        std::thread::spawn(move || {
            let start = std::time::Instant::now();
            while !done.load(Ordering::SeqCst) {
                if start.elapsed() > PER_FILE_TIMEOUT {
                    if let Some(h) = isolate_handle.lock().unwrap().as_ref() {
                        h.terminate_execution();
                    }
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        })
    };

    let (result, _oom) = server::engine::execute_stateless(&source, config);
    done.store(true, Ordering::SeqCst);
    let _ = watchdog.join();

    let mut console_buf = Vec::new();
    for entry in tree.iter().flatten() {
        console_buf.extend_from_slice(&entry.1);
    }
    drop(tree);
    drop(db);
    let _ = std::fs::remove_dir_all(&tmp);
    let console = String::from_utf8_lossy(&console_buf);

    if let Err(e) = result {
        return Outcome::Fail(format!("execution error: {e}"));
    }

    let Some(line) = console
        .lines()
        .find_map(|l| l.split(RESULT_SENTINEL).nth(1))
    else {
        return Outcome::Fail(
            "no result sentinel (test hung or event loop drained early)".to_string(),
        );
    };
    match serde_json::from_str::<TestReport>(line.trim()) {
        Ok(report) => {
            if report.skipped.is_some() || report.failures.is_empty() {
                Outcome::Pass
            } else {
                Outcome::Fail(report.failures.join("\n  "))
            }
        }
        Err(e) => Outcome::Fail(format!("bad report JSON: {e}: {line}")),
    }
}

#[test]
fn node_core_subset_matches_expectations() {
    ensure_v8();
    let vendor = root().join("vendor");
    let update = std::env::var("NODE_COMPAT_UPDATE").is_ok();
    let filter = std::env::var("NODE_COMPAT_FILTER").ok();

    let expectations: Expectations = std::fs::read_to_string(root().join("expectations.json"))
        .ok()
        .map(|s| serde_json::from_str(&s).expect("expectations.json must parse"))
        .unwrap_or_default();

    let mut new_expectations = Expectations::new();
    let mut drift = Vec::new();
    let mut pass = 0usize;

    for test in collect_tests(&vendor) {
        let key = format!(
            "test/parallel/{}",
            test.file_name().unwrap().to_string_lossy()
        );
        if let Some(f) = &filter {
            if !key.contains(f.as_str()) {
                if let Some(e) = expectations.get(&key) {
                    new_expectations.insert(key, e.clone());
                }
                continue;
            }
        }
        if let Some(Expectation::Detail(d)) = expectations.get(&key) {
            if d.ignore {
                new_expectations.insert(key, Expectation::Detail(d.clone()));
                continue;
            }
        }

        let outcome = run_file(&test);
        let actual = match &outcome {
            Outcome::Pass => {
                pass += 1;
                Expectation::Pass(true)
            }
            Outcome::Fail(_) => Expectation::Pass(false),
        };
        if !update {
            let expected = expectations
                .get(&key)
                .cloned()
                .unwrap_or(Expectation::Pass(true));
            if expected != actual {
                let detail = match &outcome {
                    Outcome::Fail(msg) => msg.clone(),
                    Outcome::Pass => String::new(),
                };
                drift.push(format!(
                    "{key}\n  expected: {}\n  actual:   {}\n  {detail}",
                    serde_json::to_string(&expected).unwrap(),
                    serde_json::to_string(&actual).unwrap(),
                ));
            }
        }
        new_expectations.insert(key, actual);
    }

    let total = new_expectations
        .iter()
        .filter(|(_, e)| !matches!(e, Expectation::Detail(d) if d.ignore))
        .count();
    println!("node-compat: {total} tests run, {pass} passing");

    if update {
        let path = root().join("expectations.json");
        std::fs::write(
            &path,
            serde_json::to_string_pretty(&new_expectations).unwrap() + "\n",
        )
        .expect("write expectations.json");
        println!("updated {}", path.display());
        return;
    }

    assert!(
        drift.is_empty(),
        "node-compat results drifted from tests/node_compat/expectations.json \
         (re-record with NODE_COMPAT_UPDATE=1 if intentional):\n\n{}",
        drift.join("\n\n")
    );
}
