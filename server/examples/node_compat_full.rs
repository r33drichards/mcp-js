#[path = "node_compat_full/result.rs"]
mod result;
#[path = "node_compat_full/shard.rs"]
mod shard;
use result::{BroadResult, ResultStatus, ShardSummary};
use serde::Deserialize;
use server::engine::{
    fetch::FetchConfig,
    opa::{EvalMode, PolicyChain},
    ExecutionConfig,
};
use std::{
    fs::{self, OpenOptions},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Once,
    },
    time::{Duration, Instant},
};
const PRELUDE: &str = include_str!("../tests/node_compat/runner/prelude.js");
const SENTINEL: &str = "__NODE_TEST_RESULT__";
static INIT: Once = Once::new();
#[derive(Deserialize)]
struct Inventory {
    source: InventorySource,
    tests: Vec<InventoryTest>,
}
#[derive(Deserialize)]
struct InventorySource {
    commit: String,
    node_version: String,
}
#[derive(Deserialize)]
struct InventoryTest {
    path: String,
    family: String,
    profile: String,
}
#[derive(Deserialize)]
struct Report {
    skipped: Option<String>,
    failures: Vec<String>,
}
enum Outcome {
    Pass,
    Skip(String),
    Assertion(String),
    Runtime(String),
    Timeout,
    Missing,
    Invalid(String),
}
fn assemble(path: &str, body: &str) -> String {
    format!("globalThis.__NODE_TEST_PATH__={};globalThis.__NODE_TEST_NAME__={};\n{}\ntry{{(0,eval)({});}}catch(e){{if(!(e&&e.__nodeTestSkip))throw e;}}\nglobalThis.__NODE_TEST_SETTIMEOUT__(()=>console.log({:?}+globalThis.__NODE_TEST_REPORT__()),300);",serde_json::to_string(path).unwrap(),serde_json::to_string(Path::new(path).file_name().unwrap().to_string_lossy().as_ref()).unwrap(),PRELUDE,serde_json::to_string(body).unwrap(),SENTINEL)
}
fn run(path: &str, body: &str, timeout: Duration) -> Outcome {
    INIT.call_once(server::engine::initialize_v8);
    let tmp = std::env::temp_dir().join(format!(
        "mcp-node-full-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let db = match sled::open(&tmp) {
        Ok(x) => x,
        Err(e) => return Outcome::Runtime(e.to_string()),
    };
    let tree = db.open_tree("console").unwrap();
    let fetch = FetchConfig::new_with_chain(Arc::new(PolicyChain::new(vec![], EvalMode::All)));
    let config = ExecutionConfig::new(256 * 1024 * 1024)
        .console_tree(tree.clone())
        .fetch_config(&fetch);
    let handle = config.isolate_handle.clone();
    let done = Arc::new(AtomicBool::new(false));
    let timed = Arc::new(AtomicBool::new(false));
    let wd = {
        let done = done.clone();
        let timed = timed.clone();
        std::thread::spawn(move || {
            let start = Instant::now();
            while !done.load(Ordering::SeqCst) {
                if start.elapsed() > timeout {
                    timed.store(true, Ordering::SeqCst);
                    if let Some(h) = handle.lock().unwrap().as_ref() {
                        h.terminate_execution();
                    }
                    return;
                }
                std::thread::sleep(Duration::from_millis(25))
            }
        })
    };
    let (res, _) = server::engine::execute_stateless(&assemble(path, body), config);
    done.store(true, Ordering::SeqCst);
    let _ = wd.join();
    let mut bytes = vec![];
    for x in tree.iter().flatten() {
        bytes.extend_from_slice(&x.1)
    }
    drop(tree);
    drop(db);
    let _ = fs::remove_dir_all(tmp);
    if timed.load(Ordering::SeqCst) {
        return Outcome::Timeout;
    }
    if let Err(e) = res {
        let d = e.to_string();
        return if d.starts_with("AssertionError:") {
            Outcome::Assertion(d)
        } else {
            Outcome::Runtime(d)
        };
    }
    let console = String::from_utf8_lossy(&bytes);
    let Some(line) = console.lines().find_map(|x| x.split(SENTINEL).nth(1)) else {
        return Outcome::Missing;
    };
    match serde_json::from_str::<Report>(line.trim()) {
        Ok(r) if !r.failures.is_empty() => Outcome::Assertion(r.failures.join("\n")),
        Ok(r) if r.skipped.is_some() => Outcome::Skip(r.skipped.unwrap()),
        Ok(_) => Outcome::Pass,
        Err(e) => Outcome::Invalid(e.to_string()),
    }
}
fn path_env(n: &str) -> Result<PathBuf, String> {
    std::env::var_os(n)
        .map(PathBuf::from)
        .ok_or_else(|| format!("{n} required"))
}
fn num_env(n: &str) -> Result<usize, String> {
    std::env::var(n)
        .map_err(|_| format!("{n} required"))?
        .parse()
        .map_err(|_| format!("{n} integer required"))
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let inv = std::env::var_os("NODE_COMPAT_INVENTORY")
        .map(PathBuf::from)
        .unwrap_or_else(|| repo.join("server/tests/node_compat/inventory.json"));
    let corpus = path_env("NODE_COMPAT_CORPUS")?;
    let results = path_env("NODE_COMPAT_RESULTS")?;
    let summary = path_env("NODE_COMPAT_SUMMARY")?;
    let i = num_env("NODE_COMPAT_SHARD_INDEX")?;
    let n = num_env("NODE_COMPAT_SHARD_TOTAL")?;
    if n == 0 || i >= n {
        return Err("invalid shard".into());
    }
    let timeout = Duration::from_secs(
        std::env::var("NODE_COMPAT_TIMEOUT_SECONDS")
            .ok()
            .and_then(|x| x.parse().ok())
            .unwrap_or(10),
    );
    let inventory: Inventory = serde_json::from_str(&fs::read_to_string(inv)?)?;
    if let Some(p) = results.parent() {
        fs::create_dir_all(p)?
    }
    let mut out = BufWriter::new(
        OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&results)?,
    );
    let mut sum = ShardSummary::new(
        i,
        n,
        &inventory.source.commit,
        &inventory.source.node_version,
    );
    for t in inventory
        .tests
        .iter()
        .filter(|t| shard::stable_shard(&t.path, n) == i)
    {
        let start = Instant::now();
        let (status, reason, details) = match fs::read_to_string(corpus.join(&t.path)) {
            Ok(src) => match run(&t.path, &src, timeout) {
                Outcome::Pass => (ResultStatus::Pass, None, None),
                Outcome::Skip(r) if result::is_platform_inapplicable(&r) => {
                    (ResultStatus::PlatformInapplicable, Some(r), None)
                }
                Outcome::Skip(r) => (ResultStatus::Unsupported, Some(r), None),
                Outcome::Assertion(d) => (ResultStatus::AssertionFailure, None, Some(d)),
                Outcome::Runtime(d) => (ResultStatus::RuntimeError, None, Some(d)),
                Outcome::Timeout => (
                    ResultStatus::Timeout,
                    Some(format!("exceeded {} seconds", timeout.as_secs())),
                    None,
                ),
                Outcome::Missing => (
                    ResultStatus::HarnessMissing,
                    Some("no result sentinel".into()),
                    None,
                ),
                Outcome::Invalid(d) => (
                    ResultStatus::InfrastructureError,
                    Some("invalid report".into()),
                    Some(d),
                ),
            },
            Err(e) => (ResultStatus::FixtureMissing, Some(e.to_string()), None),
        };
        let r = BroadResult::new(
            t,
            &inventory.source,
            i,
            n,
            status,
            start.elapsed(),
            reason,
            details,
        );
        sum.record(&r);
        serde_json::to_writer(&mut out, &r)?;
        out.write_all(b"\n")?;
        out.flush()?
    }
    fs::write(summary, serde_json::to_string_pretty(&sum)? + "\n")?;
    println!(
        "node-compat-full shard {i}/{n}: {} results, {} failing",
        sum.total, sum.failing
    );
    if sum.failing > 0 {
        std::process::exit(1)
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn platform_skip_is_strict() {
        assert!(result::is_platform_inapplicable("Windows-only"));
        assert!(!result::is_platform_inapplicable("missing crypto"));
    }
    #[test]
    fn stable_shards() {
        assert_eq!(
            shard::stable_shard("test/parallel/a.js", 16),
            shard::stable_shard("test/parallel/a.js", 16)
        );
    }
}
