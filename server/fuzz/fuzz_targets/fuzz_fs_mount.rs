#![no_main]
//! Model-based fuzzing of the overlay mount (`fs_mount` + `fs_store` + `fs_tree`).
//!
//! An arbitrary sequence of filesystem operations is applied both to a real
//! `SessionMount` (over an in-memory blob store) and to a trivial reference
//! model (a `BTreeMap` of normalized path -> file). After every operation the
//! outcomes must agree; on `push` the produced snapshot is flattened and
//! compared against the model file-for-file, then the session continues on a
//! fresh mount pulled from that snapshot — so base-tree resolution, whiteouts,
//! structural sharing, and the upper layer are all exercised against each other.

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use server::engine::fs_mount::SessionMount;
use server::engine::fs_store::FsStore;
use server::engine::fs_tree::{components_of, path_of};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Small name pool so independently generated paths frequently collide,
/// which is what makes rename/copy/remove/readdir interactions interesting.
const NAMES: &[&str] = &["a", "b", "c", "dir", "sub", "file.txt", "x"];

#[derive(Arbitrary, Debug, Clone)]
enum Seg {
    /// Pick from the shared pool (collisions).
    Fixed(u8),
    /// Arbitrary component: may be empty, ".", "..", contain '/', unicode…
    Raw(String),
}

#[derive(Arbitrary, Debug, Clone)]
struct PathSpec {
    segs: Vec<Seg>,
}

impl PathSpec {
    fn render(&self) -> String {
        self.segs
            .iter()
            .take(5)
            .map(|s| match s {
                Seg::Fixed(i) => NAMES[*i as usize % NAMES.len()].to_string(),
                Seg::Raw(r) => r.chars().take(12).collect(),
            })
            .collect::<Vec<_>>()
            .join("/")
    }
}

#[derive(Arbitrary, Debug)]
enum Op {
    Write { path: PathSpec, data: Vec<u8>, big: bool },
    Read { path: PathSpec },
    Copy { from: PathSpec, to: PathSpec },
    Rename { from: PathSpec, to: PathSpec },
    Unlink { path: PathSpec },
    Remove { path: PathSpec, recursive: bool },
    Mkdir { path: PathSpec },
    Symlink { target: PathSpec, link: PathSpec },
    Readlink { path: PathSpec },
    Stat { path: PathSpec },
    Exists { path: PathSpec },
    Readdir { path: PathSpec },
    /// Fold the upper layer into a snapshot, verify it against the model,
    /// and continue on a fresh mount pulled from that snapshot.
    PushAndRemount,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ModelFile {
    bytes: Vec<u8>,
    symlink: Option<PathBuf>,
}

type Model = BTreeMap<PathBuf, ModelFile>;

/// Normalize a raw path string exactly the way the mount does.
fn norm(raw: &str) -> PathBuf {
    path_of(&components_of(Path::new(raw)))
}

/// Whether `path` lives strictly under directory `dir` (empty `dir` = root).
fn strictly_under(dir: &Path, path: &Path) -> bool {
    if dir.as_os_str().is_empty() {
        return !path.as_os_str().is_empty();
    }
    match path.strip_prefix(dir) {
        Ok(rest) => !rest.as_os_str().is_empty(),
        Err(_) => false,
    }
}

fn descendants(model: &Model, dir: &Path) -> Vec<PathBuf> {
    model
        .keys()
        .filter(|k| strictly_under(dir, k))
        .cloned()
        .collect()
}

/// Cycle the data past the inline threshold so writes exercise the
/// chunked (blob-backed) storage path, not just inline entries.
fn inflate(data: &[u8]) -> Vec<u8> {
    let pattern: &[u8] = if data.is_empty() { b"\x42\x99" } else { data };
    pattern.iter().copied().cycle().take(80_000).collect()
}

async fn verify_snapshot(store: &FsStore, root: blake3::Hash, model: &Model) {
    let manifest = store.get_manifest(&root).await.expect("get_manifest");
    assert_eq!(
        manifest.entries.keys().collect::<Vec<_>>(),
        model.keys().collect::<Vec<_>>(),
        "snapshot must contain exactly the model's files"
    );
    for (path, file) in model {
        let entry = &manifest.entries[path];
        assert_eq!(entry.size, file.bytes.len() as u64, "size of {path:?}");
        assert_eq!(entry.symlink, file.symlink, "symlink of {path:?}");
        assert_eq!(
            store.read_file(entry).await.expect("read_file"),
            file.bytes,
            "content of {path:?}"
        );
    }
}

fuzz_target!(|ops: Vec<Op>| {
    futures::executor::block_on(async {
        let store = FsStore::in_memory();
        let mut mount = SessionMount::empty(store.clone());
        let mut model: Model = BTreeMap::new();

        for op in ops.into_iter().take(32) {
            match op {
                Op::Write { path, data, big } => {
                    let raw = path.render();
                    let key = norm(&raw);
                    if key.as_os_str().is_empty() {
                        continue; // a file at the root path is not representable
                    }
                    let bytes = if big {
                        inflate(&data)
                    } else {
                        let mut d = data;
                        d.truncate(4096);
                        d
                    };
                    mount.write(Path::new(&raw), &bytes).await.expect("write");
                    model.insert(key, ModelFile { bytes, symlink: None });
                }
                Op::Read { path } => {
                    let raw = path.render();
                    let res = mount.read(Path::new(&raw)).await;
                    match model.get(&norm(&raw)) {
                        Some(f) => assert_eq!(res.expect("read existing"), f.bytes),
                        None => assert!(res.is_err(), "read of absent {raw:?} must fail"),
                    }
                }
                Op::Copy { from, to } => {
                    let (from_raw, to_raw) = (from.render(), to.render());
                    let to_key = norm(&to_raw);
                    let src = model.get(&norm(&from_raw)).cloned();
                    if src.is_some() && to_key.as_os_str().is_empty() {
                        continue;
                    }
                    let res = mount.copy(Path::new(&from_raw), Path::new(&to_raw)).await;
                    match src {
                        Some(f) => {
                            res.expect("copy existing");
                            model.insert(to_key, f);
                        }
                        None => assert!(res.is_err(), "copy of absent {from_raw:?} must fail"),
                    }
                }
                Op::Rename { from, to } => {
                    let (from_raw, to_raw) = (from.render(), to.render());
                    let from_key = norm(&from_raw);
                    let to_key = norm(&to_raw);
                    if from_key == to_key {
                        mount
                            .rename(Path::new(&from_raw), Path::new(&to_raw))
                            .await
                            .expect("same-path rename is a no-op");
                    } else if model.contains_key(&from_key) {
                        // Plain file move.
                        if to_key.as_os_str().is_empty() {
                            continue;
                        }
                        mount
                            .rename(Path::new(&from_raw), Path::new(&to_raw))
                            .await
                            .expect("file rename");
                        let f = model.remove(&from_key).unwrap();
                        model.insert(to_key, f);
                    } else if strictly_under(&from_key, &to_key)
                        || strictly_under(&to_key, &from_key)
                    {
                        // Directory renames with overlapping subtrees are
                        // rejected: into its own subtree (EINVAL) and onto its
                        // own ancestor (ENOTEMPTY).
                        assert!(mount
                            .rename(Path::new(&from_raw), Path::new(&to_raw))
                            .await
                            .is_err());
                    } else {
                        let desc = descendants(&model, &from_key);
                        let res = mount.rename(Path::new(&from_raw), Path::new(&to_raw)).await;
                        if desc.is_empty() {
                            assert!(res.is_err(), "rename of absent {from_raw:?} must fail");
                        } else {
                            res.expect("directory rename");
                            for d in desc {
                                let rest = d.strip_prefix(&from_key).unwrap().to_path_buf();
                                let f = model.remove(&d).unwrap();
                                model.insert(to_key.join(rest), f);
                            }
                        }
                    }
                }
                Op::Unlink { path } => {
                    let raw = path.render();
                    let res = mount.unlink(Path::new(&raw)).await;
                    if model.remove(&norm(&raw)).is_some() {
                        res.expect("unlink existing");
                    } else {
                        assert!(res.is_err(), "unlink of absent {raw:?} must fail");
                    }
                }
                Op::Remove { path, recursive } => {
                    let raw = path.render();
                    let key = norm(&raw);
                    let res = mount.remove(Path::new(&raw), recursive).await;
                    if model.contains_key(&key) {
                        res.expect("remove file");
                        model.remove(&key);
                    } else {
                        let desc = descendants(&model, &key);
                        if desc.is_empty() {
                            assert!(res.is_err(), "remove of absent {raw:?} must fail");
                        } else if !recursive {
                            assert!(res.is_err(), "non-recursive dir remove must fail");
                        } else {
                            res.expect("recursive remove");
                            for d in desc {
                                model.remove(&d);
                            }
                        }
                    }
                }
                Op::Mkdir { path } => {
                    // Directories are implicit; mkdir always succeeds.
                    mount.mkdir(Path::new(&path.render())).await.expect("mkdir");
                }
                Op::Symlink { target, link } => {
                    let (target_raw, link_raw) = (target.render(), link.render());
                    let link_key = norm(&link_raw);
                    if link_key.as_os_str().is_empty() {
                        continue;
                    }
                    mount
                        .symlink(Path::new(&target_raw), Path::new(&link_raw))
                        .await
                        .expect("symlink");
                    model.insert(
                        link_key,
                        ModelFile {
                            bytes: target_raw.clone().into_bytes(),
                            symlink: Some(PathBuf::from(&target_raw)),
                        },
                    );
                }
                Op::Readlink { path } => {
                    let raw = path.render();
                    let res = mount.readlink(Path::new(&raw)).await;
                    match model.get(&norm(&raw)) {
                        Some(ModelFile { symlink: Some(t), .. }) => {
                            assert_eq!(res.expect("readlink"), *t)
                        }
                        Some(_) => assert!(res.is_err(), "readlink of non-symlink must fail"),
                        None => assert!(res.is_err(), "readlink of absent {raw:?} must fail"),
                    }
                }
                Op::Stat { path } => {
                    let raw = path.render();
                    let key = norm(&raw);
                    let res = mount.stat(Path::new(&raw)).await;
                    match model.get(&key) {
                        Some(f) => {
                            let st = res.expect("stat file");
                            assert!(!st.is_dir);
                            assert_eq!(st.size, f.bytes.len() as u64);
                            assert_eq!(st.symlink, f.symlink);
                            let want_mode = if f.symlink.is_some() { 0o120777 } else { 0o644 };
                            assert_eq!(st.mode, want_mode);
                        }
                        None if !descendants(&model, &key).is_empty() => {
                            assert!(res.expect("stat dir").is_dir);
                        }
                        None => assert!(res.is_err(), "stat of absent {raw:?} must fail"),
                    }
                }
                Op::Exists { path } => {
                    let raw = path.render();
                    let key = norm(&raw);
                    let expected =
                        model.contains_key(&key) || !descendants(&model, &key).is_empty();
                    assert_eq!(mount.exists(Path::new(&raw)).await, expected, "exists {raw:?}");
                }
                Op::Readdir { path } => {
                    let raw = path.render();
                    let key = norm(&raw);
                    let res = mount.readdir(Path::new(&raw)).await;
                    let expected: Vec<String> = model
                        .keys()
                        .filter(|k| strictly_under(&key, k))
                        .filter_map(|k| {
                            k.strip_prefix(&key)
                                .ok()?
                                .components()
                                .next()
                                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                        })
                        .collect::<std::collections::BTreeSet<_>>()
                        .into_iter()
                        .collect();
                    if expected.is_empty() {
                        if key.as_os_str().is_empty() {
                            assert_eq!(res.expect("readdir root"), Vec::<String>::new());
                        } else {
                            assert!(res.is_err(), "readdir of absent {raw:?} must fail");
                        }
                    } else {
                        assert_eq!(res.expect("readdir"), expected, "readdir {raw:?}");
                    }
                }
                Op::PushAndRemount => {
                    let root = mount.push().await.expect("push");
                    verify_snapshot(&store, root, &model).await;
                    mount = SessionMount::pull(store.clone(), root).await.expect("pull");
                    // Pushing an untouched mount must reproduce the same root.
                    assert_eq!(
                        mount.push().await.expect("re-push"),
                        root,
                        "push must be deterministic"
                    );
                }
            }
        }

        // Final fold: whatever state we ended in must snapshot correctly.
        let root = mount.push().await.expect("final push");
        verify_snapshot(&store, root, &model).await;
    });
});
