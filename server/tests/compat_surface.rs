//! Phase-0 compat surface scan (see docs/compat-test-suites-research.md).
//!
//! Executes `tools/compat/surface_scan.js` inside the engine with the
//! always-on extensions plus fetch and fs enabled, and checks the reported
//! global surface against the checked-in baseline
//! `tests/wpt/surface_baseline.json`. The baseline is the contract: adding
//! or removing a global is fine, but must be recorded deliberately.
//!
//! Re-record with:
//!
//!   COMPAT_SURFACE_UPDATE=1 cargo test --test compat_surface -- --nocapture
//!
//! The scan also reports coverage of the WinterTC Minimum Common Web
//! Platform API (Ecma TC55) — the roadmap checklist for web-API compat.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Once};

use server::engine::fs::FsConfig;
use server::engine::opa::{EvalMode, PolicyChain};
use server::engine::{fetch::FetchConfig, initialize_v8, ExecutionConfig};

static INIT: Once = Once::new();

fn ensure_v8() {
    INIT.call_once(initialize_v8);
}

const SCAN_JS: &str = include_str!("../../tools/compat/surface_scan.js");
const SENTINEL: &str = "__SURFACE__";

fn baseline_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/wpt/surface_baseline.json")
}

fn run_scan() -> serde_json::Value {
    let tmp = std::env::temp_dir().join(format!(
        "mcp-surface-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let db = sled::open(&tmp).expect("open sled db");
    let tree = db.open_tree("console").expect("open tree");

    let allow_all = || Arc::new(PolicyChain::new(vec![], EvalMode::All));
    let fetch_config = FetchConfig::new_with_chain(allow_all());
    let fs_config = FsConfig::new(allow_all());

    let config = ExecutionConfig::new(64 * 1024 * 1024)
        .console_tree(tree.clone())
        .fetch_config(&fetch_config)
        .maybe_fs_config(Some(&fs_config));
    let (result, _oom) = server::engine::execute_stateless(SCAN_JS, config);
    assert!(result.is_ok(), "surface scan failed to execute: {result:?}");

    let mut buf = Vec::new();
    for entry in tree.iter().flatten() {
        buf.extend_from_slice(&entry.1);
    }
    let _ = std::fs::remove_dir_all(&tmp);
    let console = String::from_utf8_lossy(&buf);

    let json = console
        .lines()
        .find_map(|l| l.trim().strip_prefix(SENTINEL))
        .expect("surface scan must emit sentinel line");
    serde_json::from_str(json).expect("surface report must be valid JSON")
}

#[test]
fn surface_matches_baseline() {
    ensure_v8();
    let report = run_scan();

    let min = &report["minCommonApi"];
    println!(
        "WinterTC Minimum Common API coverage: {}/{} globals present",
        min["present"], min["total"]
    );
    println!("missing: {}", min["missing"]);

    // Invariants: the surface agents rely on today must exist regardless of
    // what the baseline says.
    let globals = &report["globals"];
    for name in [
        "console", "fetch", "setTimeout", "clearTimeout", "atob", "btoa",
        "TextEncoder", "TextDecoder", "Blob", "File", "FormData",
        "WebAssembly", "fs",
    ] {
        assert!(
            !globals[name].is_null(),
            "expected global `{name}` is missing from the runtime surface"
        );
    }

    let path = baseline_path();
    if std::env::var("COMPAT_SURFACE_UPDATE").is_ok() {
        std::fs::write(&path, serde_json::to_string_pretty(&report).unwrap() + "\n")
            .expect("write surface baseline");
        println!("updated {}", path.display());
        return;
    }

    let baseline: serde_json::Value = match std::fs::read_to_string(&path) {
        Ok(s) => serde_json::from_str(&s).expect("surface_baseline.json must parse"),
        Err(_) => panic!(
            "missing {} — record it with COMPAT_SURFACE_UPDATE=1",
            path.display()
        ),
    };

    // Compare global names only (not typeof details) so V8 version bumps
    // that add builtins are surfaced but incidental value changes are not.
    let names = |v: &serde_json::Value| -> Vec<String> {
        v["globals"]
            .as_object()
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default()
    };
    let (actual, expected) = (names(&report), names(&baseline));
    let added: Vec<_> = actual.iter().filter(|n| !expected.contains(n)).collect();
    let removed: Vec<_> = expected.iter().filter(|n| !actual.contains(n)).collect();
    assert!(
        added.is_empty() && removed.is_empty(),
        "global surface drifted from tests/wpt/surface_baseline.json \
         (re-record with COMPAT_SURFACE_UPDATE=1 if intentional)\n\
         added: {added:?}\nremoved: {removed:?}"
    );
}
