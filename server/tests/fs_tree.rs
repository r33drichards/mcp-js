//! The recursive content-addressed tree behind the mount: structural sharing
//! (a push rewrites only the changed spine) and a differential test proving the
//! lazy tree-backed mount matches a flat reference model across random edits and
//! a push/pull round-trip.

use server::engine::fs_mount::SessionMount;
use server::engine::fs_store::FsStore;
use std::collections::{BTreeMap, BTreeSet};

/// Push `files` through a fresh empty mount; return the snapshot root id.
async fn snapshot(store: &FsStore, files: &[(&str, &[u8])]) -> [u8; 32] {
    let mut m = SessionMount::empty(store.clone());
    for (p, d) in files {
        m.write(p.as_ref(), d).await.unwrap();
    }
    *m.push().await.unwrap().as_bytes()
}

/// Changing one subtree must leave every other subtree's node hash untouched, so
/// the new snapshot shares all unchanged nodes with the old one.
#[tokio::test]
async fn push_shares_unchanged_subtrees() {
    let store = FsStore::in_memory();
    let r1 = snapshot(&store, &[("a/x", b"1"), ("a/y", b"2"), ("b/z", b"3")]).await;

    // Pull r1, change only a/x, push r2.
    let mut m = SessionMount::pull(store.clone(), blake3::Hash::from_bytes(r1))
        .await
        .unwrap();
    m.write("a/x".as_ref(), b"changed").await.unwrap();
    let r2 = *m.push().await.unwrap().as_bytes();

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

    // The untouched subtree `b` is the very same node in both snapshots…
    assert_eq!(
        dir_hash(r1, "b").await,
        dir_hash(r2, "b").await,
        "unchanged subtree must be shared (identical node hash)"
    );
    // …while the changed subtree `a` is a different node, and so is the root.
    assert_ne!(dir_hash(r1, "a").await, dir_hash(r2, "a").await);
    assert_ne!(r1, r2);
}

/// An empty snapshot is a real, stable root (its single empty node), and pulling
/// it yields an empty directory rather than an error.
#[tokio::test]
async fn empty_snapshot_is_canonical() {
    let store = FsStore::in_memory();
    let a = *SessionMount::empty(store.clone())
        .push()
        .await
        .unwrap()
        .as_bytes();
    let b = snapshot(&store, &[]).await;
    assert_eq!(a, b, "all empty snapshots share one canonical root");

    let m = SessionMount::pull(store.clone(), blake3::Hash::from_bytes(a))
        .await
        .unwrap();
    assert_eq!(m.readdir("".as_ref()).await.unwrap(), Vec::<String>::new());
}

// ── Differential test: tree-backed mount vs flat reference ──────────────────

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
    fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[(self.next() as usize) % xs.len()]
    }
}

/// The flat reference: path -> bytes. All file paths are leaves (no file is a
/// prefix of another), so there is never a file/dir name collision and the tree
/// and the reference agree exactly.
type Ref = BTreeMap<String, Vec<u8>>;

fn ref_dir_children(r: &Ref, dir: &str) -> BTreeSet<String> {
    let prefix = if dir.is_empty() {
        String::new()
    } else {
        format!("{dir}/")
    };
    let mut out = BTreeSet::new();
    for p in r.keys() {
        if let Some(rest) = p.strip_prefix(&prefix) {
            if !rest.is_empty() && (dir.is_empty() == prefix.is_empty()) {
                if let Some(first) = rest.split('/').next() {
                    out.insert(first.to_string());
                }
            }
        }
    }
    out
}

fn ref_has_under(r: &Ref, dir: &str) -> bool {
    let prefix = format!("{dir}/");
    r.keys().any(|p| p.starts_with(&prefix))
}

/// Assert the mount reflects the reference exactly for every known path and dir.
async fn assert_consistent(m: &SessionMount, r: &Ref, files: &[&str], dirs: &[&str]) {
    for &p in files {
        match r.get(p) {
            Some(bytes) => {
                assert_eq!(&m.read(p.as_ref()).await.unwrap(), bytes, "read {p}");
                assert!(m.exists(p.as_ref()).await, "exists {p}");
                let st = m.stat(p.as_ref()).await.unwrap();
                assert!(!st.is_dir && st.size as usize == bytes.len(), "stat {p}");
            }
            None => {
                assert!(m.read(p.as_ref()).await.is_err(), "absent read {p}");
                assert!(!m.exists(p.as_ref()).await, "absent exists {p}");
            }
        }
    }
    for &d in dirs {
        if ref_has_under(r, d) {
            let mut got = m.readdir(d.as_ref()).await.unwrap();
            got.sort();
            let want: Vec<String> = ref_dir_children(r, d).into_iter().collect();
            assert_eq!(got, want, "readdir {d}");
            assert!(m.stat(d.as_ref()).await.unwrap().is_dir, "stat dir {d}");
        } else {
            assert!(m.readdir(d.as_ref()).await.is_err(), "absent dir readdir {d}");
        }
    }
    // Root always lists, even when empty.
    let mut root = m.readdir("".as_ref()).await.unwrap();
    root.sort();
    let want: Vec<String> = ref_dir_children(r, "").into_iter().collect();
    assert_eq!(root, want, "readdir root");
}

#[tokio::test]
async fn mount_matches_flat_reference_through_push_pull() {
    let store = FsStore::in_memory();
    let dirs = ["a", "b", "c"];
    let files = [
        "a/f0", "a/f1", "a/sub/g", "b/f0", "b/f1", "c/f0", "c/sub/h", "r0",
    ];

    let mut rng = Rng(0x9E3779B97F4A7C15);

    // Phase 1: build a base snapshot from a random subset.
    let mut reference: Ref = BTreeMap::new();
    let mut base = SessionMount::empty(store.clone());
    for &p in &files {
        if rng.next() % 2 == 0 {
            let content = format!("base:{p}").into_bytes();
            base.write(p.as_ref(), &content).await.unwrap();
            reference.insert(p.to_string(), content);
        }
    }
    let root = base.push().await.unwrap();

    // Phase 2: random edits on a mount pulled from the base; mirror each on the
    // reference and assert full consistency after every step.
    let mut m = SessionMount::pull(store.clone(), root).await.unwrap();
    assert_consistent(&m, &reference, &files, &dirs).await;

    for i in 0..400u64 {
        match rng.next() % 4 {
            0 => {
                // write
                let p = *rng.pick(&files);
                let content = format!("v{i}:{p}").into_bytes();
                m.write(p.as_ref(), &content).await.unwrap();
                reference.insert(p.to_string(), content);
            }
            1 => {
                // remove a file (present -> Ok, absent -> Err)
                let p = *rng.pick(&files);
                let present = reference.remove(p).is_some();
                let res = m.remove(p.as_ref(), false).await;
                // A leaf path: absent removal is ENOENT. (Skip the case where p
                // is also a directory prefix — our files are all leaves.)
                if present {
                    assert!(res.is_ok(), "remove present {p}");
                } else if !ref_has_under(&reference, p) {
                    assert!(res.is_err(), "remove absent {p}");
                }
            }
            2 => {
                // recursive remove of a directory
                let d = *rng.pick(&dirs);
                let had = ref_has_under(&reference, d);
                let res = m.remove(d.as_ref(), true).await;
                reference.retain(|k, _| !k.starts_with(&format!("{d}/")));
                if had {
                    assert!(res.is_ok(), "rmdir {d}");
                } else {
                    assert!(res.is_err(), "rmdir absent {d}");
                }
            }
            _ => {
                // re-create a file to keep dirs populated
                let p = *rng.pick(&files);
                let content = format!("re{i}:{p}").into_bytes();
                m.write(p.as_ref(), &content).await.unwrap();
                reference.insert(p.to_string(), content);
            }
        }
        assert_consistent(&m, &reference, &files, &dirs).await;
    }

    // Phase 3: push and pull a fresh mount — the persisted tree must reproduce
    // the reference exactly.
    let new_root = m.push().await.unwrap();
    let m2 = SessionMount::pull(store.clone(), new_root).await.unwrap();
    assert_consistent(&m2, &reference, &files, &dirs).await;
}

// ── Streaming chunker + copy-by-reference ───────────────────────────────────

use server::engine::fs_store::Content;

fn pseudo_random(len: usize, seed: u64) -> Vec<u8> {
    let mut s = seed | 1;
    (0..len)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            (s >> 24) as u8
        })
        .collect()
}

#[tokio::test]
async fn large_file_streams_and_round_trips() {
    let store = FsStore::in_memory();
    let big = pseudo_random(3 * 1024 * 1024, 0xABCD); // 3 MiB -> chunked

    // Ingest straight from a streaming reader (bounded memory).
    let entry = store
        .put_file_stream(std::io::Cursor::new(big.clone()))
        .await
        .unwrap();
    assert!(matches!(entry.content, Content::Chunks(_)), "large file must chunk");
    assert_eq!(store.read_file(&entry).await.unwrap(), big);

    // And through the mount (writeFile path) + push/pull.
    let mut m = SessionMount::empty(store.clone());
    m.write("d/big.bin".as_ref(), &big).await.unwrap();
    let root = m.push().await.unwrap();
    let m2 = SessionMount::pull(store.clone(), root).await.unwrap();
    assert_eq!(m2.read("d/big.bin".as_ref()).await.unwrap(), big);
}

#[tokio::test]
async fn copy_is_by_reference_no_rechunk() {
    let store = FsStore::in_memory();
    let big = pseudo_random(2 * 1024 * 1024, 0x1234);

    let mut m = SessionMount::empty(store.clone());
    m.write("a/big".as_ref(), &big).await.unwrap();
    m.copy("a/big".as_ref(), "b/copy".as_ref()).await.unwrap();

    // Both readable and identical.
    assert_eq!(m.read("a/big".as_ref()).await.unwrap(), big);
    assert_eq!(m.read("b/copy".as_ref()).await.unwrap(), big);

    // After push, both paths point at the very same chunk list — the copy moved
    // the entry, it did not re-chunk the bytes.
    let snap = store
        .get_manifest(&m.push().await.unwrap())
        .await
        .unwrap();
    assert_eq!(
        snap.entries["a/big".as_ref() as &std::path::Path].content,
        snap.entries["b/copy".as_ref() as &std::path::Path].content,
    );
    assert!(matches!(
        snap.entries["a/big".as_ref() as &std::path::Path].content,
        Content::Chunks(_)
    ));
}
