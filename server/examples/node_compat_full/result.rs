use super::{InventorySource, InventoryTest};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, time::Duration};
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum ResultStatus {
    Pass,
    AssertionFailure,
    RuntimeError,
    HarnessMissing,
    PolicyRequired,
    Unsupported,
    Timeout,
    Crash,
    FixtureMissing,
    PlatformInapplicable,
    InfrastructureError,
}
impl ResultStatus {
    pub fn failing(&self) -> bool {
        !matches!(self, Self::Pass | Self::PlatformInapplicable)
    }
}
#[derive(Debug, Serialize, Deserialize)]
pub struct BroadResult {
    pub schema_version: u32,
    pub path: String,
    pub node_version: String,
    pub corpus_commit: String,
    pub platform: String,
    pub arch: String,
    pub shard_index: usize,
    pub shard_total: usize,
    pub family: String,
    pub profile: String,
    pub status: ResultStatus,
    pub duration_ms: u64,
    pub reason: Option<String>,
    pub details: Option<String>,
}
impl BroadResult {
    pub(super) fn new(
        t: &InventoryTest,
        s: &InventorySource,
        i: usize,
        n: usize,
        status: ResultStatus,
        d: Duration,
        reason: Option<String>,
        details: Option<String>,
    ) -> Self {
        Self {
            schema_version: 1,
            path: t.path.clone(),
            node_version: s.node_version.clone(),
            corpus_commit: s.commit.clone(),
            platform: "linux".into(),
            arch: "x86_64".into(),
            shard_index: i,
            shard_total: n,
            family: t.family.clone(),
            profile: t.profile.clone(),
            status,
            duration_ms: d.as_millis().try_into().unwrap_or(u64::MAX),
            reason,
            details,
        }
    }
}
#[derive(Debug, Serialize, Deserialize)]
pub struct ShardSummary {
    pub schema_version: u32,
    pub shard_index: usize,
    pub shard_total: usize,
    pub corpus_commit: String,
    pub node_version: String,
    pub total: usize,
    pub failing: usize,
    pub status: BTreeMap<String, usize>,
}
impl ShardSummary {
    pub fn new(i: usize, n: usize, c: &str, v: &str) -> Self {
        Self {
            schema_version: 1,
            shard_index: i,
            shard_total: n,
            corpus_commit: c.into(),
            node_version: v.into(),
            total: 0,
            failing: 0,
            status: BTreeMap::new(),
        }
    }
    pub fn record(&mut self, r: &BroadResult) {
        self.total += 1;
        if r.status.failing() {
            self.failing += 1
        }
        let k = serde_json::to_value(&r.status)
            .unwrap()
            .as_str()
            .unwrap()
            .to_string();
        *self.status.entry(k).or_default() += 1;
    }
}
pub fn is_platform_inapplicable(reason: &str) -> bool {
    let r = reason.to_ascii_lowercase();
    [
        "windows",
        "win32",
        "macos",
        "darwin",
        "aix",
        "freebsd",
        "openbsd",
        "sunos",
        "solaris",
        "ibmi",
        "ppc64",
        "s390",
        "arm64-only",
        "32-bit only",
    ]
    .iter()
    .any(|x| r.contains(x))
}
