#![no_main]
//! Differential fuzzing of the two snapshot-merge implementations.
//!
//! `merge_manifests` (flat, Mercurial-style) and `merge_trees` (lazy,
//! hash-pruned recursive walk) implement the same per-path 3-way rule and must
//! agree exactly: same merged file set and same conflicts. Manifests are built
//! from small name/content pools so the three sides frequently share paths and
//! entries, hitting every branch of the rule (identical, one-side-changed,
//! diverged, add/add, modify/delete).

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use server::engine::fs_merge::{merge_manifests, merge_trees, MergeConflict, Prefer};
use server::engine::fs_store::{Content, Entry, FsStore, Manifest};
use server::engine::fs_tree::{components_of, path_of};
use std::path::Path;

const NAMES: &[&str] = &["a", "b", "c", "d", "sub", "f.txt"];
const CONTENTS: &[&[u8]] = &[b"", b"one", b"two", b"three"];

#[derive(Arbitrary, Debug, Clone)]
struct FileSpec {
    segs: Vec<u8>,
    content: u8,
    custom: Option<Vec<u8>>,
    executable: bool,
}

fn build_manifest(files: &[FileSpec]) -> Manifest {
    let mut m = Manifest::default();
    for f in files.iter().take(16) {
        let raw: String = f
            .segs
            .iter()
            .take(4)
            .map(|i| NAMES[*i as usize % NAMES.len()])
            .collect::<Vec<_>>()
            .join("/");
        let path = path_of(&components_of(Path::new(&raw)));
        if path.as_os_str().is_empty() {
            continue;
        }
        let bytes: Vec<u8> = match &f.custom {
            Some(b) => b.iter().copied().take(64).collect(),
            None => CONTENTS[f.content as usize % CONTENTS.len()].to_vec(),
        };
        let entry = Entry {
            mode: if f.executable { 0o755 } else { 0o644 },
            size: bytes.len() as u64,
            content: Content::Inline(bytes),
            symlink: None,
        };
        m.entries.insert(path, entry);
    }
    m
}

#[derive(Arbitrary, Debug)]
enum ArbPrefer {
    None,
    Ours,
    Theirs,
}

#[derive(Arbitrary, Debug)]
struct MergeInput {
    base: Option<Vec<FileSpec>>,
    ours: Vec<FileSpec>,
    theirs: Vec<FileSpec>,
    prefer: ArbPrefer,
}

fn sorted_conflicts(mut cs: Vec<MergeConflict>) -> Vec<MergeConflict> {
    cs.sort_by(|a, b| a.path.cmp(&b.path));
    cs
}

fuzz_target!(|input: MergeInput| {
    let base = input.base.as_deref().map(build_manifest);
    let ours = build_manifest(&input.ours);
    let theirs = build_manifest(&input.theirs);
    let prefer = match input.prefer {
        ArbPrefer::None => Prefer::None,
        ArbPrefer::Ours => Prefer::Ours,
        ArbPrefer::Theirs => Prefer::Theirs,
    };

    let flat = merge_manifests(base.as_ref(), &ours, &theirs, prefer);

    // ── Local invariants of the flat merge ──────────────────────────────
    if prefer != Prefer::None {
        assert!(flat.conflicts.is_empty(), "prefer must resolve all conflicts");
    }
    for (path, entry) in &flat.merged.entries {
        let o = ours.entries.get(path);
        let t = theirs.entries.get(path);
        assert!(
            o == Some(entry) || t == Some(entry),
            "merged entry at {path:?} must come from one of the sides"
        );
    }
    for c in &flat.conflicts {
        assert!(
            !flat.merged.entries.contains_key(&c.path),
            "conflicting path {:?} must not appear in the merged result",
            c.path
        );
        assert_ne!(c.ours, c.theirs, "a conflict requires diverged sides");
    }

    // Merging identical sides is the identity, regardless of base.
    let same = merge_manifests(base.as_ref(), &ours, &ours, prefer);
    assert!(same.conflicts.is_empty());
    assert_eq!(same.merged.entries, ours.entries);

    // The rule is symmetric in ours/theirs when no side is preferred.
    if prefer == Prefer::None {
        let swapped = merge_manifests(base.as_ref(), &theirs, &ours, prefer);
        assert_eq!(swapped.merged.entries, flat.merged.entries, "merge must be symmetric");
        let mut swapped_back = swapped.conflicts;
        for c in &mut swapped_back {
            std::mem::swap(&mut c.ours, &mut c.theirs);
        }
        assert_eq!(
            sorted_conflicts(swapped_back),
            sorted_conflicts(flat.conflicts.clone()),
            "conflicts must be symmetric"
        );
    }

    // ── Differential: the lazy tree merge must agree with the flat merge ──
    futures::executor::block_on(async {
        let store = FsStore::in_memory();
        let base_root = match &base {
            Some(m) => Some(*store.put_manifest(m).await.expect("put base").as_bytes()),
            None => None,
        };
        let ours_root = *store.put_manifest(&ours).await.expect("put ours").as_bytes();
        let theirs_root = *store.put_manifest(&theirs).await.expect("put theirs").as_bytes();

        let out = merge_trees(&store, base_root, Some(ours_root), Some(theirs_root), prefer)
            .await
            .expect("merge_trees");
        let tree_merged = store
            .get_manifest(&blake3::Hash::from_bytes(out.root))
            .await
            .expect("flatten merged tree");

        assert_eq!(
            tree_merged.entries, flat.merged.entries,
            "tree merge and flat merge must produce the same files"
        );
        assert_eq!(
            sorted_conflicts(out.conflicts),
            sorted_conflicts(flat.conflicts),
            "tree merge and flat merge must report the same conflicts"
        );
    });
});
