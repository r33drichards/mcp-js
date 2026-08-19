//! Node.js core test harness.
//!
//! Runs the vendored subset of nodejs/node test/parallel files (see
//! tools/compat/vendor-node-tests.sh) inside the real engine via
//! `execute_stateless`, against the `node:` compat modules served by the
//! module loader. Each file runs in a fresh isolate: an ESM prelude
//! provides the CJS shell (require over the node: registry, the
//! `../common` module with mustCall tracking, process/Buffer globals),
//! then the test body runs through a CommonJS function wrapper and a
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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ExpectationStatus {
    Pass,
    Fail,
    Unsupported,
    HarnessMissing,
    PolicyRequired,
    Flaky,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum CompatibilityLevel {
    Exact,
    Adapted,
    Unsupported,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct Expectation {
    status: ExpectationStatus,
    family: String,
    profile: String,
    compatibility: CompatibilityLevel,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expires: Option<String>,
}

impl Expectation {
    fn passing(family: &str, profile: &str) -> Self {
        Self {
            status: ExpectationStatus::Pass,
            family: family.to_string(),
            profile: profile.to_string(),
            compatibility: CompatibilityLevel::Exact,
            reason: None,
            expires: None,
        }
    }

    fn validate(&self) -> Result<(), String> {
        if self.family.trim().is_empty() {
            return Err("family must not be empty".to_string());
        }
        if self.profile.trim().is_empty() {
            return Err("profile must not be empty".to_string());
        }
        if self.status != ExpectationStatus::Pass
            && self
                .reason
                .as_deref()
                .is_none_or(|reason| reason.trim().is_empty())
        {
            return Err(format!("{:?} expectations require a reason", self.status));
        }
        if self.status == ExpectationStatus::Flaky
            && self
                .expires
                .as_deref()
                .is_none_or(|expires| expires.trim().is_empty())
        {
            return Err("flaky expectations require an expiry date".to_string());
        }
        if self.status == ExpectationStatus::Pass
            && self.compatibility == CompatibilityLevel::Unsupported
        {
            return Err("passing expectations cannot be unsupported".to_string());
        }
        Ok(())
    }

    fn runnable(&self) -> bool {
        matches!(
            self.status,
            ExpectationStatus::Pass | ExpectationStatus::Fail
        )
    }

    fn matches(&self, family: Option<&str>, profile: Option<&str>) -> bool {
        family.is_none_or(|value| self.family == value)
            && profile.is_none_or(|value| self.profile == value)
    }
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
    // Run the test in Node's CommonJS function shell so top-level return
    // and module-scoped declarations behave like a real .js test file.
    source.push_str(&format!(
        "try {{\n  globalThis.__NODE_TEST_RUN_CJS__({});\n}} catch (e) {{\n\
         \x20 if (!(e && e.__nodeTestSkip)) throw e;\n}}\n",
        serde_json::to_string(&body).unwrap()
    ));
    // Report after timers drain, via the prelude's stashed setTimeout so
    // tests that delete the timer globals can still report. 300ms keeps the
    // loop alive past the repeating-timer tests: the runtime implements the
    // HTML spec's nesting clamp (a timer nested more than 5 deep gets a 4ms
    // floor), so test-timers-non-integer-delay's 50 ticks of a ~1ms
    // interval take ~190ms here versus ~55ms in Node.
    source.push_str(&format!(
        "globalThis.__NODE_TEST_SETTIMEOUT__(() => {{ console.log({:?} + globalThis.__NODE_TEST_REPORT__()); }}, 300);\n",
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

    let fetch_config =
        FetchConfig::new_with_chain(Arc::new(PolicyChain::new(vec![], EvalMode::All)));
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
    let family_filter = std::env::var("NODE_COMPAT_FAMILY").ok();
    let profile_filter = std::env::var("NODE_COMPAT_PROFILE").ok();

    let expectations: Expectations = std::fs::read_to_string(root().join("expectations.json"))
        .ok()
        .map(|s| serde_json::from_str(&s).expect("expectations.json must parse"))
        .unwrap_or_default();
    for (key, expectation) in &expectations {
        expectation
            .validate()
            .unwrap_or_else(|error| panic!("invalid expectation for {key}: {error}"));
    }

    let mut new_expectations = expectations.clone();
    let mut drift = Vec::new();
    let mut pass = 0usize;
    let mut run = 0usize;
    let mut classified = 0usize;

    for test in collect_tests(&vendor) {
        let key = format!(
            "test/parallel/{}",
            test.file_name().unwrap().to_string_lossy()
        );
        if filter.as_deref().is_some_and(|value| !key.contains(value)) {
            continue;
        }

        let expected = expectations
            .get(&key)
            .cloned()
            .unwrap_or_else(|| Expectation::passing("other", "pure"));
        if !expected.matches(family_filter.as_deref(), profile_filter.as_deref()) {
            continue;
        }
        if !expected.runnable() {
            classified += 1;
            continue;
        }

        run += 1;
        let outcome = run_file(&test);
        let actual_status = match &outcome {
            Outcome::Pass => {
                pass += 1;
                ExpectationStatus::Pass
            }
            Outcome::Fail(_) => ExpectationStatus::Fail,
        };

        if !update && expected.status != actual_status {
            let detail = match &outcome {
                Outcome::Fail(msg) => msg.clone(),
                Outcome::Pass => String::new(),
            };
            drift.push(format!(
                "{key}\n  expected status: {:?}\n  actual status:   {:?}\n  {detail}",
                expected.status, actual_status,
            ));
        }

        if update {
            let mut updated = expected;
            updated.status = actual_status;
            if actual_status == ExpectationStatus::Pass {
                updated.reason = None;
                updated.expires = None;
            } else if updated.reason.is_none() {
                updated.reason = Some(
                    "observed failure; inspect before committing updated expectations".to_string(),
                );
            }
            new_expectations.insert(key, updated);
        }
    }

    println!("node-compat: {run} tests run, {pass} passing, {classified} classified non-runnable");

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

#[cfg(test)]
mod expectation_tests {
    use super::*;

    #[test]
    fn assemble_runs_test_body_through_commonjs_wrapper() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wrapper.js");
        std::fs::write(&path, "return;").unwrap();

        let source = assemble(&path).unwrap();

        let runner = source.split_once(PRELUDE_JS).unwrap().1;
        assert!(runner.contains("globalThis.__NODE_TEST_RUN_CJS__("));
        assert!(!runner.contains("(0, eval)("));
    }

    #[test]
    fn parses_passing_expectation_metadata() {
        let value = r#"{
          "status":"pass",
          "family":"events",
          "profile":"pure",
          "compatibility":"exact"
        }"#;
        let parsed: Expectation = serde_json::from_str(value).unwrap();
        assert_eq!(parsed.status, ExpectationStatus::Pass);
        assert_eq!(parsed.family, "events");
        assert_eq!(parsed.profile, "pure");
        assert_eq!(parsed.compatibility, CompatibilityLevel::Exact);
    }

    #[test]
    fn rejects_non_pass_without_reason() {
        let value = r#"{
          "status":"harness_missing",
          "family":"events",
          "profile":"pure",
          "compatibility":"unsupported"
        }"#;
        let parsed: Expectation = serde_json::from_str(value).unwrap();
        assert!(parsed.validate().is_err());
    }

    #[test]
    fn filter_matches_family_and_profile() {
        let expectation = Expectation::passing("events", "pure");
        assert!(expectation.matches(Some("events"), Some("pure")));
        assert!(!expectation.matches(Some("streams"), Some("pure")));
    }
}
