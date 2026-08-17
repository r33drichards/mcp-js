//! Web Platform Tests harness for the mcp-v8 engine.
//!
//! Runs the vendored WPT subset (`tests/wpt/vendor/`, pinned in
//! `tests/wpt/versions.json`) inside the engine via `execute_stateless`, the
//! same substrate the `run_js` tool uses, and compares per-subtest results
//! against the checked-in `tests/wpt/expectations.json`.
//!
//! Model (see docs/compat-test-suites-research.md): Node core's WPT runner —
//! no HTTP server, `// META: script=` includes resolved from the vendored
//! tree, a mock `location`, and a status file recording known failures. The
//! expectation format follows Deno's:
//!
//!   "path.any.js": true                          // harness OK, all subtests pass
//!   "path.any.js": false                         // file fails wholesale
//!   "path.any.js": {"expectedFailures": [...]}   // named subtests fail, rest pass
//!   "path.any.js": {"ignore": true}              // not run
//!
//! CI fails on drift in either direction (regressions AND unexpected passes).
//! Re-record after a runtime change with:
//!
//!   WPT_UPDATE=1 cargo test --test wpt_harness -- --nocapture
//!
//! Filter to a subset of files with WPT_FILTER=<substring>.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Once};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use server::engine::opa::{EvalMode, PolicyChain};
use server::engine::{fetch::FetchConfig, initialize_v8, ExecutionConfig};

static INIT: Once = Once::new();

fn ensure_v8() {
    INIT.call_once(initialize_v8);
}

const BOOTSTRAP_JS: &str = include_str!("wpt/runner/bootstrap.js");
const REPORT_JS: &str = include_str!("wpt/runner/report.js");
const RESULT_SENTINEL: &str = "__WPT_RESULT__";
/// Wall-clock cap per test file; hung isolates are terminated.
const PER_FILE_TIMEOUT: Duration = Duration::from_secs(60);

fn wpt_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/wpt")
}

// ── Expectations file ───────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
enum Expectation {
    /// `true` = all subtests pass; `false` = file fails wholesale.
    All(bool),
    Detail(ExpectationDetail),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
struct ExpectationDetail {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    ignore: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[serde(rename = "expectedFailures")]
    expected_failures: Vec<String>,
}

type Expectations = BTreeMap<String, Expectation>;

fn load_expectations() -> Expectations {
    let path = wpt_root().join("expectations.json");
    match std::fs::read_to_string(&path) {
        Ok(s) => serde_json::from_str(&s).expect("expectations.json must parse"),
        Err(_) => Expectations::new(),
    }
}

// ── Result of one test file ─────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct HarnessResult {
    status: i64,
    message: Option<String>,
    tests: Vec<SubtestResult>,
}

#[derive(Debug, Deserialize)]
struct SubtestResult {
    name: String,
    status: i64,
    message: Option<String>,
}

#[derive(Debug)]
enum FileOutcome {
    /// Harness completed: subtest failures listed (empty = clean pass).
    Ran { failures: Vec<(String, String)> },
    /// Execution error, missing completion callback, or harness error.
    Failed { reason: String },
}

// ── Source assembly ─────────────────────────────────────────────────────

/// Resolve a `// META: script=` include against the vendored tree.
/// Absolute paths (`/wasm/jsapi/assertions.js`) resolve from the vendor
/// root; relative paths resolve from the test file's directory.
fn resolve_script(vendor: &Path, test_rel: &Path, spec: &str) -> PathBuf {
    if let Some(abs) = spec.strip_prefix('/') {
        vendor.join(abs)
    } else {
        vendor.join(test_rel.parent().unwrap()).join(spec)
    }
}

fn assemble_source(vendor: &Path, test_rel: &Path) -> Result<String, String> {
    let test_path = vendor.join(test_rel);
    let body = std::fs::read_to_string(&test_path)
        .map_err(|e| format!("read {}: {e}", test_path.display()))?;

    let mut scripts = Vec::new();
    for line in body.lines() {
        let Some(rest) = line.strip_prefix("// META:") else {
            // META lines only appear in the leading comment block.
            if !line.starts_with("//") && !line.trim().is_empty() {
                break;
            }
            continue;
        };
        if let Some(spec) = rest.trim().strip_prefix("script=") {
            let path = resolve_script(vendor, test_rel, spec.trim());
            let src = std::fs::read_to_string(&path)
                .map_err(|e| format!("META script {}: {e}", path.display()))?;
            scripts.push(src);
        }
    }

    let harness = std::fs::read_to_string(vendor.join("resources/testharness.js"))
        .map_err(|e| format!("read testharness.js: {e}"))?;

    let mut source = String::new();
    source.push_str(&format!(
        "globalThis.__WPT_TEST_PATH__ = {};\n",
        serde_json::to_string(&format!("/{}", test_rel.display())).unwrap()
    ));
    source.push_str(BOOTSTRAP_JS);
    source.push_str(&harness);
    source.push_str(REPORT_JS);
    for s in &scripts {
        source.push_str(s);
        source.push('\n');
    }
    source.push_str(&body);
    Ok(source)
}

// ── Execution ───────────────────────────────────────────────────────────

fn console_tree() -> (sled::Tree, PathBuf) {
    let tmp = std::env::temp_dir().join(format!(
        "mcp-wpt-console-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let db = sled::open(&tmp).expect("open sled db");
    let tree = db.open_tree("console").expect("open tree");
    (tree, tmp)
}

fn run_file(vendor: &Path, test_rel: &Path) -> FileOutcome {
    let source = match assemble_source(vendor, test_rel) {
        Ok(s) => s,
        Err(e) => return FileOutcome::Failed { reason: e },
    };

    let (tree, tmp) = console_tree();
    let fetch_config = FetchConfig::new_with_chain(Arc::new(PolicyChain::new(
        vec![],
        EvalMode::All,
    )));

    let config = ExecutionConfig::new(256 * 1024 * 1024)
        .console_tree(tree.clone())
        .fetch_config(&fetch_config);
    let isolate_handle = config.isolate_handle.clone();

    // Watchdog: terminate the isolate if a file wedges the event loop.
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
    let _ = std::fs::remove_dir_all(&tmp);
    let console = String::from_utf8_lossy(&console_buf);

    let harness_result = console
        .lines()
        .rev()
        .find_map(|l| l.trim().strip_prefix(RESULT_SENTINEL))
        .and_then(|json| serde_json::from_str::<HarnessResult>(json).ok());

    match (result, harness_result) {
        (_, Some(hr)) if hr.status == 0 => {
            let failures = hr
                .tests
                .into_iter()
                .filter(|t| t.status != 0)
                .map(|t| {
                    let detail = t.message.unwrap_or_default();
                    (t.name, format!("status {}: {}", t.status, detail))
                })
                .collect();
            FileOutcome::Ran { failures }
        }
        (_, Some(hr)) => FileOutcome::Failed {
            reason: format!(
                "harness status {}: {}",
                hr.status,
                hr.message.unwrap_or_default()
            ),
        },
        (Err(e), None) => FileOutcome::Failed {
            reason: format!("execution error: {e}"),
        },
        (Ok(_), None) => FileOutcome::Failed {
            reason: "no completion callback (test hung or event loop drained early)".into(),
        },
    }
}

// ── Discovery ───────────────────────────────────────────────────────────

fn collect_tests(vendor: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![vendor.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("read vendor dir").flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(".any.js"))
            {
                out.push(path.strip_prefix(vendor).unwrap().to_path_buf());
            }
        }
    }
    out.sort();
    out
}

// ── The test ────────────────────────────────────────────────────────────

#[test]
fn wpt_subset_matches_expectations() {
    ensure_v8();
    let root = wpt_root();
    let vendor = root.join("vendor");
    let update = std::env::var("WPT_UPDATE").is_ok();
    let filter = std::env::var("WPT_FILTER").ok();
    let expectations = load_expectations();

    let mut new_expectations = Expectations::new();
    let mut drift: Vec<String> = Vec::new();
    let mut pass_files = 0usize;
    let mut total_subtest_failures = 0usize;

    let tests = collect_tests(&vendor);
    assert!(!tests.is_empty(), "no vendored tests found in {}", vendor.display());

    for rel in &tests {
        let key = rel.to_string_lossy().replace('\\', "/");
        if let Some(f) = &filter {
            if !key.contains(f.as_str()) {
                // Keep prior expectation so partial runs don't erase entries.
                if let Some(e) = expectations.get(&key) {
                    new_expectations.insert(key, e.clone());
                }
                continue;
            }
        }

        let expected = expectations.get(&key);
        if let Some(Expectation::Detail(d)) = expected {
            if d.ignore {
                new_expectations.insert(key, Expectation::Detail(d.clone()));
                continue;
            }
        }

        let outcome = run_file(&vendor, rel);
        let actual = match &outcome {
            FileOutcome::Ran { failures } if failures.is_empty() => {
                pass_files += 1;
                Expectation::All(true)
            }
            FileOutcome::Ran { failures } => {
                total_subtest_failures += failures.len();
                let mut names: Vec<String> =
                    failures.iter().map(|(n, _)| n.clone()).collect();
                names.sort();
                names.dedup();
                Expectation::Detail(ExpectationDetail {
                    ignore: false,
                    expected_failures: names,
                })
            }
            FileOutcome::Failed { .. } => Expectation::All(false),
        };

        if !update {
            let expected = expected.cloned().unwrap_or(Expectation::All(true));
            if expected != actual {
                let detail = match &outcome {
                    FileOutcome::Failed { reason } => reason.clone(),
                    FileOutcome::Ran { failures } => failures
                        .iter()
                        .map(|(n, m)| format!("  FAIL {n} — {m}"))
                        .collect::<Vec<_>>()
                        .join("\n"),
                };
                drift.push(format!(
                    "{key}\n  expected: {}\n  actual:   {}\n{detail}",
                    serde_json::to_string(&expected).unwrap(),
                    serde_json::to_string(&actual).unwrap(),
                ));
            }
        }
        new_expectations.insert(key, actual);
    }

    let run_count = new_expectations
        .iter()
        .filter(|(_, e)| !matches!(e, Expectation::Detail(d) if d.ignore))
        .count();
    println!(
        "WPT: {run_count} files run, {pass_files} fully passing, \
         {total_subtest_failures} subtest failures recorded"
    );

    if update {
        let path = root.join("expectations.json");
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
        "WPT results drifted from tests/wpt/expectations.json \
         (re-record with WPT_UPDATE=1 if intentional):\n\n{}",
        drift.join("\n\n")
    );
}
