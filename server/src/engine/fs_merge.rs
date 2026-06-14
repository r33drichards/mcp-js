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

use crate::engine::fs_store::{Entry, Manifest};
use std::collections::BTreeSet;
use std::path::PathBuf;

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
