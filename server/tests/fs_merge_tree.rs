//! The lazy tree merge (`merge_trees`) must produce exactly the same result as
//! the flat reference merge (`merge_manifests`) for all inputs, while pruning
//! unchanged subtrees (structural sharing) so it does not load the whole tree.

use server::engine::fs_merge::{merge_manifests, merge_trees, MergeConflict, Prefer};
use server::engine::fs_store::{Entry, FsStore, Manifest};
use std::collections::BTreeMap;
use std::path::PathBuf;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
}

fn sorted(mut c: Vec<MergeConflict>) -> Vec<MergeConflict> {
    c.sort_by(|a, b| a.path.cmp(&b.path));
    c
}

/// Build a tree from a flat manifest and return its root id.
async fn root_of(store: &FsStore, m: &Manifest) -> [u8; 32] {
    *store.put_manifest(m).await.unwrap().as_bytes()
}

#[tokio::test]
async fn tree_merge_matches_flat_merge_for_random_inputs() {
    let store = FsStore::in_memory();
    // Leaf paths spanning several directories (some nested) so the merge has
    // real subtrees to prune or descend.
    let paths = [
        "a/x", "a/y", "a/sub/p", "a/sub/q", "b/x", "b/y", "c", "d/deep/leaf",
    ];
    // A small content alphabet, so "same content on both sides" happens often.
    let contents: [&[u8]; 4] = [b"AAA", b"BBB", b"CCC", b"DDD"];

    let mut rng = Rng(0xDEADBEEFCAFEF00D);

    // Pre-make the Entry for each (content) once.
    let mut entry_for: BTreeMap<&[u8], Entry> = BTreeMap::new();
    for c in contents {
        entry_for.insert(c, store.put_file(c).await.unwrap());
    }

    let make_manifest = |rng: &mut Rng, entry_for: &BTreeMap<&[u8], Entry>| -> Manifest {
        let mut m = Manifest::default();
        for p in paths {
            // 0 => absent, else one of the contents
            let pick = rng.next() % (contents.len() as u64 + 1);
            if pick > 0 {
                let c = contents[(pick - 1) as usize];
                m.entries.insert(PathBuf::from(p), entry_for[c].clone());
            }
        }
        m
    };

    for _ in 0..150 {
        let base = make_manifest(&mut rng, &entry_for);
        let ours = make_manifest(&mut rng, &entry_for);
        let theirs = make_manifest(&mut rng, &entry_for);
        let with_base = rng.next() % 2 == 0;
        let prefer = match rng.next() % 3 {
            0 => Prefer::None,
            1 => Prefer::Ours,
            _ => Prefer::Theirs,
        };

        let base_ref = if with_base { Some(&base) } else { None };
        let flat = merge_manifests(base_ref, &ours, &theirs, prefer);

        let base_root = if with_base {
            Some(root_of(&store, &base).await)
        } else {
            None
        };
        let outcome = merge_trees(
            &store,
            base_root,
            Some(root_of(&store, &ours).await),
            Some(root_of(&store, &theirs).await),
            prefer,
        )
        .await
        .unwrap();

        // Same conflicts (paths + each side's entry).
        assert_eq!(
            sorted(outcome.conflicts.clone()),
            sorted(flat.conflicts.clone()),
            "conflicts diverged"
        );

        // Same structurally-merged result: flatten the merged tree and compare.
        let tree_merged = store
            .get_manifest(&blake3::Hash::from_bytes(outcome.root))
            .await
            .unwrap();
        assert_eq!(tree_merged, flat.merged, "merged result diverged");
    }
}

/// A merge that only touches one subtree must reuse the unchanged subtree's node
/// verbatim — proof it was pruned, not rebuilt.
#[tokio::test]
async fn tree_merge_shares_unchanged_subtrees() {
    let store = FsStore::in_memory();

    let mk = |paths: &[(&str, &[u8])]| {
        let mut m = Manifest::default();
        for (p, c) in paths {
            // Inline entry built inline to keep the helper sync-free below.
            m.entries.insert(
                PathBuf::from(*p),
                Entry {
                    mode: 0o644,
                    size: c.len() as u64,
                    content: server::engine::fs_store::Content::Inline(c.to_vec()),
                    symlink: None,
                },
            );
        }
        m
    };

    let base = mk(&[("a/x", b"1"), ("b/keep", b"shared")]);
    let ours = mk(&[("a/x", b"ours"), ("b/keep", b"shared")]);
    let theirs = mk(&[("a/x", b"1"), ("b/keep", b"shared"), ("a/y", b"new")]);

    let base_root = root_of(&store, &base).await;
    let ours_root = root_of(&store, &ours).await;
    let theirs_root = root_of(&store, &theirs).await;

    let outcome = merge_trees(
        &store,
        Some(base_root),
        Some(ours_root),
        Some(theirs_root),
        Prefer::None,
    )
    .await
    .unwrap();
    assert!(outcome.conflicts.is_empty(), "should merge cleanly");

    // The untouched `b` subtree is the same node in base and in the merged tree.
    let dir_hash = |root: [u8; 32], name: &str| {
        let store = store.clone();
        let name = name.to_string();
        async move {
            store
                .resolve(Some(root), &[name])
                .await
                .unwrap()
                .and_then(|c| c.dir)
        }
    };
    assert_eq!(
        dir_hash(base_root, "b").await,
        dir_hash(outcome.root, "b").await,
        "unchanged subtree must be shared through the merge"
    );

    // And the merge is correct: a/x from ours, a/y added from theirs.
    let merged = store
        .get_manifest(&blake3::Hash::from_bytes(outcome.root))
        .await
        .unwrap();
    assert_eq!(
        merged.entries[&PathBuf::from("a/x")].content,
        server::engine::fs_store::Content::Inline(b"ours".to_vec())
    );
    assert!(merged.entries.contains_key(&PathBuf::from("a/y")));
}
