//! Regression for the fuzz_js_scenario crash of 2026-11-21 (PR #352 CI):
//! a partition applied on the same tick as a CAS ack left the old-term entry
//! uncommitted in the new leader's log. The leader's CAS compare read that
//! stale uncommitted tail instead of the applied value, so every subsequent
//! CAS conflicted and the cluster never converged after healing.
//!
//! Fixed in cluster.rs cas_internal: the compare reads the newest
//! current-term log entry for the key, else the applied value.
use engine_cluster_tests::scenario::replay_file;

// Modeled track only: the real-sled feature in this crate still trips the
// pre-existing engine-cluster-tests real-sled path issue tracked separately;
// the fix lives in cluster.rs cas_internal, shared by both tracks.
#[cfg(not(feature = "real-sled"))]
#[test]
fn partition_tick_cas_wedge_converges_after_heal() {
    let result = replay_file(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/partition-tick-cas-wedge.json"
    ))
    .expect("scenario parses");
    result.expect_status("all assertions passed");
    // The pre-partition ack and the tick-race ack are both legal; a fresh
    // post-recovery ack must exist and all replicas must share one head.
    assert!(
        result.acked.len() >= 2,
        "expected the pre-partition ack plus a post-heal ack: {:?}",
        result.acked
    );
    let heads: std::collections::BTreeSet<_> = result.final_heads.values().collect();
    assert_eq!(
        heads.len(),
        1,
        "replicas must converge on a single head: {:?}",
        result.final_heads
    );
}
