//! Flat three-way manifest merge: per-path rules, conflicts, prefer, 2-way.

use server::engine::fs_merge::{merge_manifests, MergeOutcome, Prefer};
use server::engine::fs_store::{Content, Entry, Manifest};
use std::path::PathBuf;

/// A trivially-distinct inline entry, so different byte payloads are unequal.
fn entry(bytes: &[u8]) -> Entry {
    Entry {
        mode: 0o644,
        size: bytes.len() as u64,
        content: Content::Inline(bytes.to_vec()),
        symlink: None,
    }
}

fn manifest(pairs: &[(&str, &[u8])]) -> Manifest {
    let mut m = Manifest::default();
    for (p, b) in pairs {
        m.entries.insert(PathBuf::from(p), entry(b));
    }
    m
}

fn merged_or_panic(o: MergeOutcome) -> Manifest {
    match o {
        MergeOutcome::Merged(m) => m,
        MergeOutcome::Conflicts(c) => panic!("expected clean merge, got conflicts: {c:?}"),
    }
}

fn get(m: &Manifest, p: &str) -> Option<Vec<u8>> {
    m.entries.get(&PathBuf::from(p)).map(|e| match &e.content {
        Content::Inline(b) => b.clone(),
        Content::Chunks(_) => panic!("unexpected chunked entry in test"),
    })
}

#[test]
fn non_overlapping_changes_merge_cleanly() {
    let base = manifest(&[("a", b"a0"), ("b", b"b0")]);
    let ours = manifest(&[("a", b"a1"), ("b", b"b0")]); // changed a
    let theirs = manifest(&[("a", b"a0"), ("b", b"b1")]); // changed b
    let m = merged_or_panic(merge_manifests(Some(&base), &ours, &theirs, Prefer::None));
    assert_eq!(get(&m, "a"), Some(b"a1".to_vec()));
    assert_eq!(get(&m, "b"), Some(b"b1".to_vec()));
}

#[test]
fn identical_change_on_both_sides_is_not_a_conflict() {
    let base = manifest(&[("a", b"a0")]);
    let ours = manifest(&[("a", b"a1")]);
    let theirs = manifest(&[("a", b"a1")]);
    let m = merged_or_panic(merge_manifests(Some(&base), &ours, &theirs, Prefer::None));
    assert_eq!(get(&m, "a"), Some(b"a1".to_vec()));
}

#[test]
fn additions_on_each_side_both_survive() {
    let base = manifest(&[]);
    let ours = manifest(&[("only_ours", b"x")]);
    let theirs = manifest(&[("only_theirs", b"y")]);
    let m = merged_or_panic(merge_manifests(Some(&base), &ours, &theirs, Prefer::None));
    assert_eq!(get(&m, "only_ours"), Some(b"x".to_vec()));
    assert_eq!(get(&m, "only_theirs"), Some(b"y".to_vec()));
}

#[test]
fn one_sided_delete_is_applied() {
    let base = manifest(&[("a", b"a0"), ("b", b"b0")]);
    let ours = manifest(&[("a", b"a0")]); // deleted b
    let theirs = manifest(&[("a", b"a0"), ("b", b"b0")]); // unchanged
    let m = merged_or_panic(merge_manifests(Some(&base), &ours, &theirs, Prefer::None));
    assert_eq!(get(&m, "a"), Some(b"a0".to_vec()));
    assert_eq!(get(&m, "b"), None, "deletion should win when other side is unchanged");
}

#[test]
fn divergent_same_path_conflicts() {
    let base = manifest(&[("a", b"a0")]);
    let ours = manifest(&[("a", b"a1")]);
    let theirs = manifest(&[("a", b"a2")]);
    match merge_manifests(Some(&base), &ours, &theirs, Prefer::None) {
        MergeOutcome::Conflicts(c) => {
            assert_eq!(c.len(), 1);
            assert_eq!(c[0].path, PathBuf::from("a"));
            assert!(c[0].base.is_some() && c[0].ours.is_some() && c[0].theirs.is_some());
        }
        other => panic!("expected conflict, got {other:?}"),
    }
}

#[test]
fn modify_delete_is_a_conflict() {
    let base = manifest(&[("a", b"a0")]);
    let ours = manifest(&[("a", b"a1")]); // modified
    let theirs = manifest(&[]); // deleted
    match merge_manifests(Some(&base), &ours, &theirs, Prefer::None) {
        MergeOutcome::Conflicts(c) => {
            assert_eq!(c.len(), 1);
            assert!(c[0].ours.is_some());
            assert!(c[0].theirs.is_none());
        }
        other => panic!("expected modify/delete conflict, got {other:?}"),
    }
}

#[test]
fn prefer_ours_and_theirs_resolve_conflicts() {
    let base = manifest(&[("a", b"a0")]);
    let ours = manifest(&[("a", b"a1")]);
    let theirs = manifest(&[("a", b"a2")]);

    let m = merged_or_panic(merge_manifests(Some(&base), &ours, &theirs, Prefer::Ours));
    assert_eq!(get(&m, "a"), Some(b"a1".to_vec()));

    let m = merged_or_panic(merge_manifests(Some(&base), &ours, &theirs, Prefer::Theirs));
    assert_eq!(get(&m, "a"), Some(b"a2".to_vec()));
}

#[test]
fn two_way_merge_without_base_conflicts_on_divergence_but_unions_disjoint() {
    // No base: identical or one-sided auto-resolve; both-present-and-differ conflicts.
    let ours = manifest(&[("a", b"a1"), ("only_ours", b"x")]);
    let theirs = manifest(&[("a", b"a2"), ("only_theirs", b"y")]);
    match merge_manifests(None, &ours, &theirs, Prefer::None) {
        MergeOutcome::Conflicts(c) => {
            assert_eq!(c.len(), 1);
            assert_eq!(c[0].path, PathBuf::from("a"));
            assert!(c[0].base.is_none());
        }
        other => panic!("expected one conflict on 'a', got {other:?}"),
    }
    // With prefer, the disjoint additions are still both kept.
    let m = merged_or_panic(merge_manifests(None, &ours, &theirs, Prefer::Ours));
    assert_eq!(get(&m, "only_ours"), Some(b"x".to_vec()));
    assert_eq!(get(&m, "only_theirs"), Some(b"y".to_vec()));
    assert_eq!(get(&m, "a"), Some(b"a1".to_vec()));
}

#[test]
fn identical_inputs_merge_to_same_tree() {
    let ours = manifest(&[("a", b"a0"), ("b", b"b0")]);
    let theirs = manifest(&[("a", b"a0"), ("b", b"b0")]);
    let m = merged_or_panic(merge_manifests(None, &ours, &theirs, Prefer::None));
    assert_eq!(get(&m, "a"), Some(b"a0".to_vec()));
    assert_eq!(get(&m, "b"), Some(b"b0".to_vec()));
}
