//! Content-addressed recursive directory tree for the snapshottable filesystem.
//!
//! A snapshot root is a [`TreeNode`]; each node maps one path *component* to a
//! [`TreeChild`]. A child may carry a file `Entry`, a sub-tree hash, or **both**
//! — the latter preserves the flat-manifest quirk where a single name is at once
//! a file and a directory prefix (`{"a", "a/b"}`), so the tree round-trips any
//! flat manifest exactly.
//!
//! Nodes are keyed by the blake3 hash of their `bincode` encoding, so:
//!   * identical subtrees share one blob (dedup), and
//!   * a `push` rewrites only the nodes on the path-to-root spine of a change —
//!     every untouched subtree keeps its existing hash (structural sharing).
//!
//! This is what lets the mount load and rewrite a huge tree in O(touched), not
//! O(total): see [`crate::engine::fs_mount`] and [`crate::engine::fs_store`].

use crate::engine::fs_store::Entry;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

/// Blob-name prefix for tree nodes; keeps them from colliding with heap or
/// chunk blobs in a shared backend and lets GC recognise what it scans.
pub const TREE_PREFIX: &str = "fstree:";

pub fn tree_key(hash: &[u8; 32]) -> String {
    let mut s = String::with_capacity(TREE_PREFIX.len() + 64);
    s.push_str(TREE_PREFIX);
    for b in hash {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// One named entry in a directory node. At least one of `file`/`dir` is set; an
/// all-`None` child is never stored (the name is dropped instead).
///
/// Both being set is legal and meaningful: the name is simultaneously a file and
/// a directory prefix, exactly as a flat `BTreeMap<PathBuf, Entry>` permits.
//
// NOTE: fields are plain `Option`s with no `skip_serializing_if` — bincode is a
// non-self-describing format and cannot skip fields, so a `None` must still emit
// its one tag byte for deserialization to line up.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Debug, Default)]
pub struct TreeChild {
    /// A file living at this name.
    pub file: Option<Entry>,
    /// Hash of the sub-`TreeNode` rooted at this name.
    pub dir: Option<[u8; 32]>,
}

impl TreeChild {
    pub fn is_empty(&self) -> bool {
        self.file.is_none() && self.dir.is_none()
    }
}

/// A directory: its immediate children keyed by single path component. A
/// `BTreeMap` keeps the encoding canonical so equal trees hash equal.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Debug, Default)]
pub struct TreeNode {
    pub children: BTreeMap<String, TreeChild>,
}

/// Split a normalized path into its component names. Leading `/`, `.`, and `..`
/// are resolved the same way the mount normalizes paths.
pub fn components_of(p: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for c in p.components() {
        match c {
            Component::Normal(s) => out.push(s.to_string_lossy().into_owned()),
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir | Component::RootDir | Component::Prefix(_) => {}
        }
    }
    out
}

/// Rebuild a `PathBuf` from component names.
pub fn path_of(comps: &[String]) -> PathBuf {
    let mut p = PathBuf::new();
    for c in comps {
        p.push(c);
    }
    p
}
