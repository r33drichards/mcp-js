//! Mark-and-sweep garbage collection for the fs object store.
//!
//! Roots are the current head of every label plus every CA id still in every
//! label's reflog (within the retention window). From each root manifest we mark
//! the manifest blob and every entry's chunk blobs; any fs blob in the backend
//! that ends up unmarked is swept.
//!
//! Safety property: a label that was `reset` past a snapshot keeps that snapshot
//! reachable through its reflog, so roll-forward still works. Only once the
//! reflog entry falls outside the retention window does the snapshot become
//! collectable.

use std::collections::HashSet;

use crate::engine::fs_labels::{CaId, LabelStore};
use crate::engine::fs_store::{chunk_key, Content, FsStore};
use crate::engine::fs_tree::tree_key;

/// Which reflog entries count as GC roots.
#[derive(Clone, Copy, Debug)]
pub enum ReflogRetention {
    /// Every reflog entry is a root (nothing rolls out of reach).
    KeepAll,
    /// Only the most recent `n` entries per label are roots; older snapshots
    /// referenced solely by pruned entries become collectable.
    KeepLast(usize),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GcStats {
    pub roots: usize,
    pub marked: usize,
    pub deleted: usize,
}

/// Run a full mark-and-sweep over the fs object store. Only blobs the fs store
/// itself wrote (manifest / chunk prefixes) are ever considered for deletion, so
/// a backend shared with the heap store stays safe.
pub async fn collect(
    store: &FsStore,
    labels: &LabelStore,
    retention: ReflogRetention,
) -> Result<GcStats, String> {
    // ── Roots ────────────────────────────────────────────────────────────
    let mut roots: HashSet<CaId> = HashSet::new();
    for (name, head) in labels.list().await? {
        roots.insert(head);
        let log = labels.log(&name).await?;
        let kept: &[_] = match retention {
            ReflogRetention::KeepAll => &log,
            ReflogRetention::KeepLast(n) => {
                let start = log.len().saturating_sub(n);
                &log[start..]
            }
        };
        // Root on each retained entry's resulting snapshot (`to`). An entry's
        // `from` is always some earlier entry's `to`, so including it would
        // defeat retention pruning (a rolled-past id would never age out).
        for entry in kept {
            roots.insert(entry.to);
        }
    }

    // ── Mark ─────────────────────────────────────────────────────────────
    let mut marked: HashSet<String> = HashSet::new();
    for root in &roots {
        mark_tree(store, root, &mut marked).await?;
    }

    // ── Sweep ────────────────────────────────────────────────────────────
    let mut deleted = 0usize;
    for key in store.blobs().list().await? {
        if !is_fs_blob(&key) {
            continue; // never touch non-fs blobs in a shared backend
        }
        if !marked.contains(&key) {
            store
                .blobs()
                .delete(&key)
                .await
                .map_err(|e| format!("gc delete {key}: {e}"))?;
            deleted += 1;
        }
    }

    Ok(GcStats {
        roots: roots.len(),
        marked: marked.len(),
        deleted,
    })
}

/// Walk the snapshot tree rooted at `id`, marking every tree-node blob and the
/// chunk blobs of every file entry. Shared subtrees are visited once (the
/// `marked` set short-circuits a repeated node hash), so the walk is O(reachable
/// nodes), not O(snapshots × tree). A missing node (a root that was never
/// materialized, or a partial subtree) is skipped, not fatal.
async fn mark_tree(
    store: &FsStore,
    id: &CaId,
    marked: &mut HashSet<String>,
) -> Result<(), String> {
    let mut stack: Vec<[u8; 32]> = vec![*id];
    while let Some(node_id) = stack.pop() {
        if !marked.insert(tree_key(&node_id)) {
            continue; // already visited this (possibly shared) subtree
        }
        let node = match store.get_node(&node_id).await {
            Ok(n) => n,
            Err(_) => continue,
        };
        for child in node.children.values() {
            if let Some(entry) = &child.file {
                if let Content::Chunks(hashes) = &entry.content {
                    for h in hashes {
                        marked.insert(chunk_key(h));
                    }
                }
            }
            if let Some(d) = &child.dir {
                stack.push(*d);
            }
        }
    }
    Ok(())
}

fn is_fs_blob(key: &str) -> bool {
    key.starts_with("fschunk:") || key.starts_with("fstree:") || key.starts_with("fsmanifest:")
}
