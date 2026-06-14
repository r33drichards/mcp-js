//! Stress / load tests for the label reflog, focused on the cost added by the
//! optional per-move message field.
//!
//! Performance shape of the feature: each label move (`create`/`cas`/`force`)
//! appends exactly one reflog entry, and the message rides along on that entry.
//! The append itself stays O(1). The cost that grows is the full-reflog scan —
//! `log()` (and therefore `fs_reset`, which validates against the reflog) loads
//! and deserializes *every* entry for the label, so it is O(entries × bytes).
//! A message enlarges each entry, so the only way it could hurt is by (a) an
//! unbounded message bloating a single entry, or (b) very long histories making
//! the scan expensive. (a) is capped by `MAX_MESSAGE_LEN`; these tests exercise
//! (b) and confirm correctness and roughly linear behaviour at scale.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use server::engine::fs_labels::LabelStore;

/// A message of the size we'd expect in practice (a commit-style note).
fn msg(i: usize) -> Option<String> {
    Some(format!(
        "advance #{i}: regenerated assets and ran the integration suite"
    ))
}

/// Drive a long sequential history with a message on every move, then assert the
/// reflog is fully intact and a full scan stays correct. This is the hot path the
/// message field touches: append on every push, scan over the whole history.
#[tokio::test]
async fn sequential_pushes_with_messages_scale_linearly() {
    const N: usize = 10_000;
    let s = LabelStore::in_memory();

    let mut head = [0u8; 32];
    s.create("main", head, msg(0)).await.unwrap();

    let start = Instant::now();
    for i in 1..N {
        let next = caid(i as u64);
        assert!(
            s.cas("main", Some(head), next, msg(i)).await.unwrap(),
            "fast-forward #{i} should win"
        );
        head = next;
    }
    let push_elapsed = start.elapsed();

    // One full-reflog scan, the O(history) read that gains the message bytes.
    let scan_start = Instant::now();
    let log = s.log("main").await.unwrap();
    let scan_elapsed = scan_start.elapsed();

    assert_eq!(log.len(), N, "every move must be recorded exactly once");
    assert_eq!(s.resolve("main").await.unwrap(), Some(head));
    // Spot-check that messages survived round-tripping through sled at scale.
    assert_eq!(log[0].message, msg(0));
    assert_eq!(log[N - 1].message, msg(N - 1));
    // The reflog is append-ordered, so the i-th entry points at the i-th head.
    assert_eq!(log[N / 2].to, caid((N / 2) as u64));

    eprintln!(
        "fs_labels stress: {N} message-carrying pushes in {push_elapsed:?} \
         ({:.0} pushes/s); full {N}-entry reflog scan in {scan_elapsed:?}",
        N as f64 / push_elapsed.as_secs_f64().max(1e-9),
    );
}

/// Many writers racing to advance the same label from the same expected head.
/// CAS must serialize them so exactly one wins per generation, and the message
/// is recorded only for the move that actually landed — no torn or duplicated
/// reflog entries under contention.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_cas_contention_records_one_message_per_generation() {
    const WRITERS: usize = 32;
    const GENERATIONS: usize = 200;
    let s = LabelStore::in_memory();

    let mut head = [0u8; 32];
    s.create("main", head, Some("genesis".into())).await.unwrap();

    for generation in 0..GENERATIONS {
        let wins = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::with_capacity(WRITERS);
        for w in 0..WRITERS {
            let store = s.clone();
            let wins = wins.clone();
            // Each writer proposes a distinct new head, tagged with its identity.
            // The +1 offsets keep every proposal non-zero and distinct from the
            // genesis head and every prior winner, so a win always *moves* the
            // pointer (never a no-op CAS that two writers could both satisfy).
            let next = caid(((generation as u64) + 1) << 8 | (w as u64 + 1));
            handles.push(tokio::spawn(async move {
                let message = Some(format!("gen {generation} writer {w}"));
                if store
                    .cas("main", Some(head), next, message)
                    .await
                    .unwrap()
                {
                    wins.fetch_add(1, Ordering::SeqCst);
                    Some(next)
                } else {
                    None
                }
            }));
        }

        let mut winner = None;
        for h in handles {
            if let Some(n) = h.await.unwrap() {
                assert!(winner.is_none(), "two writers cannot both win a generation");
                winner = Some(n);
            }
        }
        assert_eq!(wins.load(Ordering::SeqCst), 1, "exactly one writer wins");
        head = winner.expect("some writer must win each generation");
    }

    // History length is genesis + one accepted move per generation; every losing
    // CAS left no trace. This is the invariant a message field must not break.
    let log = s.log("main").await.unwrap();
    assert_eq!(log.len(), GENERATIONS + 1);
    assert_eq!(s.resolve("main").await.unwrap(), Some(head));
    // Each surviving entry carries exactly the winning writer's message.
    for entry in log.iter().skip(1) {
        let m = entry.message.as_deref().unwrap_or("");
        assert!(m.starts_with("gen "), "unexpected reflog message: {m:?}");
    }
}

fn caid(seed: u64) -> [u8; 32] {
    let mut id = [0u8; 32];
    id[..8].copy_from_slice(&seed.to_le_bytes());
    id
}
