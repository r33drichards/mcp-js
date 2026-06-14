//! Flat three-way merge of two snapshot manifests.
//!
//! Our manifest is a flat `BTreeMap<PathBuf, Entry>` (no per-directory hash),
//! so the merge mirrors Mercurial's `manifestmerge` rather than git's recursive
//! tree walk: we diff the three flat maps and decide each path independently.
//! `Entry` is content-addressed, so per-path equality is the cheap short-circuit
//! that auto-resolves unchanged / identically-changed paths.
//!
//! Per-path rule (base = least-common-ancestor entry, or `None` for a 2-way
//! merge):
//!   * `ours == theirs`  -> take it (includes both-added-identical, both-deleted)
//!   * `ours == base`     -> take theirs (only theirs changed)
//!   * `theirs == base`   -> take ours (only ours changed)
//!   * otherwise          -> conflict (both diverged, or modify/delete)
//!
//! With no base, every path where both sides changed (and differ) conflicts —
//! a true 2-way merge.

use crate::engine::fs_store::{Entry, FsStore, Manifest};
use crate::engine::fs_tree::{path_of, TreeChild, TreeNode};
use std::collections::BTreeSet;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

/// How to resolve a divergent path.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Prefer {
    /// Report conflicts instead of resolving them.
    None,
    /// Auto-resolve every conflict to our side.
    Ours,
    /// Auto-resolve every conflict to their side.
    Theirs,
}

impl Prefer {
    /// Parse the surface-level `prefer` argument (`None`/`""` → report conflicts).
    pub fn parse(s: Option<&str>) -> Result<Prefer, String> {
        match s.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
            None | Some("") | Some("none") => Ok(Prefer::None),
            Some("ours") => Ok(Prefer::Ours),
            Some("theirs") => Ok(Prefer::Theirs),
            Some(other) => Err(format!("invalid prefer '{other}': expected ours|theirs|none")),
        }
    }
}

/// A path that diverged on both sides (and `prefer` was `None`). Each field is
/// the side's entry, or `None` if the file is absent on that side.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergeConflict {
    pub path: PathBuf,
    pub base: Option<Entry>,
    pub ours: Option<Entry>,
    pub theirs: Option<Entry>,
}

/// The structural result of a merge: the cleanly-resolved tree plus any paths
/// that diverged. `conflicts` is empty for a fully clean merge. A content-merge
/// pass (see `fs_content_merge`) can still reconcile some of the conflicts by
/// looking at the actual file bytes.
#[derive(Debug, Default)]
pub struct MergeResult {
    pub merged: Manifest,
    pub conflicts: Vec<MergeConflict>,
}

/// Three-way merge `ours` and `theirs` against an optional common `base`.
pub fn merge_manifests(
    base: Option<&Manifest>,
    ours: &Manifest,
    theirs: &Manifest,
    prefer: Prefer,
) -> MergeResult {
    // Union of every path across the three manifests.
    let mut paths: BTreeSet<&PathBuf> = BTreeSet::new();
    if let Some(b) = base {
        paths.extend(b.entries.keys());
    }
    paths.extend(ours.entries.keys());
    paths.extend(theirs.entries.keys());

    let mut merged = Manifest::default();
    let mut conflicts = Vec::new();

    for path in paths {
        let b = base.and_then(|m| m.entries.get(path));
        let o = ours.entries.get(path);
        let t = theirs.entries.get(path);

        // `Some(side)` picks a side (which may itself be `None` = deletion);
        // the outer `None` means "conflict".
        let chosen: Option<Option<&Entry>> = if o == t {
            Some(o) // identical on both sides (incl. both-deleted)
        } else if o == b {
            Some(t) // ours unchanged from base -> take theirs
        } else if t == b {
            Some(o) // theirs unchanged from base -> take ours
        } else {
            match prefer {
                Prefer::Ours => Some(o),
                Prefer::Theirs => Some(t),
                Prefer::None => None,
            }
        };

        match chosen {
            Some(Some(e)) => {
                merged.entries.insert(path.clone(), e.clone());
            }
            Some(None) => { /* resolved as a deletion: omit from the result */ }
            None => conflicts.push(MergeConflict {
                path: path.clone(),
                base: b.cloned(),
                ours: o.cloned(),
                theirs: t.cloned(),
            }),
        }
    }

    MergeResult { merged, conflicts }
}

// ── Lazy tree merge ─────────────────────────────────────────────────────────
//
// The same per-path 3-way rule, but over the recursive tree: equal subtrees are
// pruned by hash comparison without ever being loaded, so a merge of two
// snapshots that differ in one corner costs O(differing paths), not O(tree). The
// structurally-merged result is written as a new tree (sharing unchanged nodes);
// conflicting files are left out of it and returned for the content-merge pass,
// exactly as the flat merge does.

/// The outcome of a tree merge: the structurally-merged root (conflicting paths
/// omitted, to be patched by the caller's content merge) and the conflicts.
pub struct TreeMergeOutcome {
    pub root: [u8; 32],
    pub conflicts: Vec<MergeConflict>,
}

/// Three-way merge of snapshot roots `ours`/`theirs` against optional `base`.
pub async fn merge_trees(
    store: &FsStore,
    base: Option<[u8; 32]>,
    ours: Option<[u8; 32]>,
    theirs: Option<[u8; 32]>,
    prefer: Prefer,
) -> anyhow::Result<TreeMergeOutcome> {
    let (root, conflicts) = merge_node(store, base, ours, theirs, prefer, Vec::new()).await?;
    let root = match root {
        Some(h) => h,
        None => store.put_node(&TreeNode::default()).await?, // empty merged tree
    };
    Ok(TreeMergeOutcome { root, conflicts })
}

enum FileDecision {
    Take(Option<Entry>),
    Conflict,
}

/// The per-path 3-way rule applied to the file part of a name (mirrors
/// `merge_manifests`' inner logic).
fn decide_file(
    b: &Option<Entry>,
    o: &Option<Entry>,
    t: &Option<Entry>,
    prefer: Prefer,
) -> FileDecision {
    if o == t {
        FileDecision::Take(o.clone())
    } else if o == b {
        FileDecision::Take(t.clone())
    } else if t == b {
        FileDecision::Take(o.clone())
    } else {
        match prefer {
            Prefer::Ours => FileDecision::Take(o.clone()),
            Prefer::Theirs => FileDecision::Take(t.clone()),
            Prefer::None => FileDecision::Conflict,
        }
    }
}

async fn load_opt(
    store: &FsStore,
    id: Option<[u8; 32]>,
) -> anyhow::Result<Option<Arc<TreeNode>>> {
    match id {
        Some(h) => Ok(Some(store.get_node(&h).await?)),
        None => Ok(None),
    }
}

fn child_of(node: &Option<Arc<TreeNode>>, name: &str) -> TreeChild {
    node.as_ref()
        .and_then(|n| n.children.get(name).cloned())
        .unwrap_or_default()
}

#[allow(clippy::type_complexity)]
fn merge_node<'a>(
    store: &'a FsStore,
    base: Option<[u8; 32]>,
    ours: Option<[u8; 32]>,
    theirs: Option<[u8; 32]>,
    prefer: Prefer,
    prefix: Vec<String>,
) -> Pin<Box<dyn Future<Output = anyhow::Result<(Option<[u8; 32]>, Vec<MergeConflict>)>> + Send + 'a>>
{
    Box::pin(async move {
        // Prune whole subtrees by hash, without loading them.
        if ours == theirs {
            return Ok((ours, Vec::new())); // identical (incl. both absent)
        }
        if base == ours {
            return Ok((theirs, Vec::new())); // only theirs changed -> take theirs
        }
        if base == theirs {
            return Ok((ours, Vec::new())); // only ours changed -> take ours
        }

        let bn = load_opt(store, base).await?;
        let on = load_opt(store, ours).await?;
        let tn = load_opt(store, theirs).await?;

        let mut names: BTreeSet<String> = BTreeSet::new();
        for n in [bn.as_ref(), on.as_ref(), tn.as_ref()].into_iter().flatten() {
            names.extend(n.children.keys().cloned());
        }

        let mut out = TreeNode::default();
        let mut conflicts = Vec::new();
        for name in names {
            let bc = child_of(&bn, &name);
            let oc = child_of(&on, &name);
            let tc = child_of(&tn, &name);
            let mut child = TreeChild::default();

            match decide_file(&bc.file, &oc.file, &tc.file, prefer) {
                FileDecision::Take(f) => child.file = f,
                FileDecision::Conflict => {
                    let mut p = prefix.clone();
                    p.push(name.clone());
                    conflicts.push(MergeConflict {
                        path: path_of(&p),
                        base: bc.file.clone(),
                        ours: oc.file.clone(),
                        theirs: tc.file.clone(),
                    });
                    // file left None — patched later by the content merge.
                }
            }

            // Directory part: prune equal subtrees, else descend.
            child.dir = if oc.dir == tc.dir {
                oc.dir
            } else if bc.dir == oc.dir {
                tc.dir
            } else if bc.dir == tc.dir {
                oc.dir
            } else {
                let mut cp = prefix.clone();
                cp.push(name.clone());
                let (d, mut sub) =
                    merge_node(store, bc.dir, oc.dir, tc.dir, prefer, cp).await?;
                conflicts.append(&mut sub);
                d
            };

            if !child.is_empty() {
                out.children.insert(name, child);
            }
        }

        let id = if out.children.is_empty() {
            None
        } else {
            Some(store.put_node(&out).await?)
        };
        Ok((id, conflicts))
    })
}
