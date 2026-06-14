//! The userspace overlay mount: a read-only pinned base snapshot plus a mutable
//! upper write layer, entirely in-process. No kernel overlayfs/FUSE. `pull` pins
//! a base root hash at mount time so concurrent label moves do not shift a
//! running session; `push` folds the upper over the base into a new pure
//! snapshot and returns its root id (the durable artifact — the mount is
//! ephemeral).
//!
//! The base is **never** materialised in full: reads resolve through the
//! content-addressed tree lazily, and `push` rewrites only the changed nodes.
//! So a session over a multi-TB snapshot costs O(touched paths), not O(total).

use crate::engine::fs_store::{Entry, FsStore};
use crate::engine::fs_tree::{components_of, path_of};
use blake3::Hash;
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

/// A pending change in the upper layer.
enum Write {
    Data(Entry),
    Whiteout,
}

/// Metadata returned by [`SessionMount::stat`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stat {
    pub mode: u32,
    pub size: u64,
    pub is_dir: bool,
    pub symlink: Option<PathBuf>,
}

pub struct SessionMount {
    store: FsStore,
    base_id: Option<Hash>,          // pinned root hash at pull() time
    upper: HashMap<PathBuf, Write>, // the only mutable state
}

impl SessionMount {
    pub async fn pull(store: FsStore, id: Hash) -> anyhow::Result<Self> {
        // Lazy: only pin the root hash. The tree is walked on demand.
        Ok(Self {
            store,
            base_id: Some(id),
            upper: HashMap::new(),
        })
    }

    pub fn empty(store: FsStore) -> Self {
        Self {
            store,
            base_id: None,
            upper: HashMap::new(),
        }
    }

    pub fn base_id(&self) -> Option<Hash> {
        self.base_id
    }

    fn base_root(&self) -> Option<[u8; 32]> {
        self.base_id.map(|h| *h.as_bytes())
    }

    /// Resolve the effective file entry for a path: an upper `Data` shadows the
    /// base; an upper `Whiteout` (or a missing base entry) means "no file here".
    async fn effective(&self, comps: &[String], key: &Path) -> anyhow::Result<Option<Entry>> {
        match self.upper.get(key) {
            Some(Write::Whiteout) => Ok(None),
            Some(Write::Data(e)) => Ok(Some(e.clone())),
            None => self.store.resolve_file(self.base_root(), comps).await,
        }
    }

    pub async fn read(&self, p: &Path) -> anyhow::Result<Vec<u8>> {
        let comps = components_of(p);
        let key = path_of(&comps);
        match self.effective(&comps, &key).await? {
            Some(e) => self.store.read_file(&e).await,
            None => anyhow::bail!("ENOENT: {}", key.display()),
        }
    }

    pub async fn write(&mut self, p: &Path, bytes: &[u8]) -> anyhow::Result<()> {
        let entry = self.store.put_file(bytes).await?;
        self.upper.insert(path_of(&components_of(p)), Write::Data(entry));
        Ok(())
    }

    pub async fn unlink(&mut self, p: &Path) -> anyhow::Result<()> {
        let comps = components_of(p);
        let key = path_of(&comps);
        if self.effective(&comps, &key).await?.is_none() {
            anyhow::bail!("ENOENT: {}", key.display());
        }
        self.upper.insert(key, Write::Whiteout);
        Ok(())
    }

    /// Whether `p` names an existing file or (implicit) directory.
    pub async fn exists(&self, p: &Path) -> bool {
        let comps = components_of(p);
        let key = path_of(&comps);
        self.effective(&comps, &key).await.ok().flatten().is_some()
            || self.is_implicit_dir(&comps, &key).await.unwrap_or(false)
    }

    /// Remove a file, or a directory (recursively whiting out descendants).
    /// Mirrors Node `fs.rm`: non-recursive removal of a non-empty directory
    /// fails.
    pub async fn remove(&mut self, p: &Path, recursive: bool) -> anyhow::Result<()> {
        let comps = components_of(p);
        let key = path_of(&comps);
        if self.effective(&comps, &key).await?.is_some() {
            // Plain file.
            self.upper.insert(key, Write::Whiteout);
            return Ok(());
        }
        let descendants = self.live_descendants(&comps, &key).await?;
        if descendants.is_empty() {
            anyhow::bail!("ENOENT: {}", key.display());
        }
        if !recursive {
            anyhow::bail!("ENOTEMPTY: {}", key.display());
        }
        for d in descendants {
            self.upper.insert(d, Write::Whiteout);
        }
        Ok(())
    }

    pub async fn stat(&self, p: &Path) -> anyhow::Result<Stat> {
        let comps = components_of(p);
        let key = path_of(&comps);
        if let Some(e) = self.effective(&comps, &key).await? {
            return Ok(Stat {
                mode: e.mode,
                size: e.size,
                is_dir: false,
                symlink: e.symlink.clone(),
            });
        }
        // Directories are implicit: a path is a directory if anything lives under it.
        if self.is_implicit_dir(&comps, &key).await? {
            return Ok(Stat {
                mode: 0o755,
                size: 0,
                is_dir: true,
                symlink: None,
            });
        }
        anyhow::bail!("ENOENT: {}", key.display())
    }

    /// `mkdir` is a no-op in virtual mode: directories are implied by the paths
    /// of the files they contain. It succeeds so callers can mkdir-then-write.
    pub async fn mkdir(&mut self, _p: &Path) -> anyhow::Result<()> {
        Ok(())
    }

    pub async fn rename(&mut self, from: &Path, to: &Path) -> anyhow::Result<()> {
        let from_c = components_of(from);
        let from_k = path_of(&from_c);
        let to_c = components_of(to);
        let to_k = path_of(&to_c);
        if from_k == to_k {
            return Ok(());
        }
        // Plain file: move the single entry.
        if let Some(entry) = self.effective(&from_c, &from_k).await? {
            self.upper.insert(to_k, Write::Data(entry));
            self.upper.insert(from_k, Write::Whiteout);
            return Ok(());
        }
        // Directory (implicit): move every descendant, rewriting its path prefix
        // from `from/...` to `to/...`. Mirrors the host-backed recursive rename.
        if child_suffix(&from_k, &to_k).is_some() {
            anyhow::bail!("EINVAL: cannot rename {} into its own subtree", from_k.display());
        }
        let srcs = self.live_descendants(&from_c, &from_k).await?;
        if srcs.is_empty() {
            anyhow::bail!("ENOENT: {}", from_k.display());
        }
        for d in srcs {
            let rest = match child_suffix(&from_k, &d) {
                Some(r) => r,
                None => continue,
            };
            let d_comps = components_of(&d);
            if let Some(entry) = self.effective(&d_comps, &d).await? {
                self.upper.insert(to_k.join(&rest), Write::Data(entry));
                self.upper.insert(d, Write::Whiteout);
            }
        }
        Ok(())
    }

    /// Immediate children (file and subdirectory names) of `dir`, over base ∪
    /// upper minus whiteouts. Lists only the directory's own node plus the small
    /// upper layer — it never enumerates the whole tree.
    pub async fn readdir(&self, dir: &Path) -> anyhow::Result<Vec<String>> {
        let dir_c = components_of(dir);
        let dir_k = path_of(&dir_c);
        let mut names: BTreeSet<String> = BTreeSet::new();

        // Base immediate children, each kept only if it still has something live.
        if let Some(node) = self.store.dir_node_at(self.base_root(), &dir_c).await? {
            for name in node.children.keys() {
                let mut child_c = dir_c.clone();
                child_c.push(name.clone());
                let child_k = path_of(&child_c);
                if self.effective(&child_c, &child_k).await?.is_some()
                    || self.is_implicit_dir(&child_c, &child_k).await?
                {
                    names.insert(name.clone());
                }
            }
        }

        // Upper additions.
        for (k, w) in &self.upper {
            if let Write::Data(_) = w {
                if let Some(rest) = child_suffix(&dir_k, k) {
                    if let Some(first) = rest.components().next() {
                        names.insert(first.as_os_str().to_string_lossy().to_string());
                    }
                }
            }
        }

        if names.is_empty() {
            if dir_c.is_empty() {
                return Ok(Vec::new()); // the root always exists, possibly empty
            }
            anyhow::bail!("ENOENT: {}", dir_k.display());
        }
        Ok(names.into_iter().collect())
    }

    /// Fold the upper over the base into a new pure snapshot; return its root id.
    /// Writes only the changed tree nodes (structural sharing).
    pub async fn push(&self) -> anyhow::Result<Hash> {
        let mut changes: Vec<(Vec<String>, Option<Entry>)> = Vec::with_capacity(self.upper.len());
        for (path, w) in &self.upper {
            let comps = components_of(path);
            match w {
                Write::Data(e) => changes.push((comps, Some(e.clone()))),
                Write::Whiteout => changes.push((comps, None)),
            }
        }
        let root = self.store.build_root(self.base_root(), changes).await?;
        Ok(Hash::from_bytes(root))
    }

    /// Whether `dir` has any live descendant (so it exists as an implicit
    /// directory). Fast path: a base directory node with children and no
    /// whiteouts under it is live without enumeration; otherwise the (bounded)
    /// subtree is scanned to confirm not everything was whited out.
    async fn is_implicit_dir(&self, dir_comps: &[String], dir_key: &Path) -> anyhow::Result<bool> {
        if self
            .upper
            .iter()
            .any(|(k, w)| matches!(w, Write::Data(_)) && child_suffix(dir_key, k).is_some())
        {
            return Ok(true);
        }
        let has_white = self
            .upper
            .iter()
            .any(|(k, w)| matches!(w, Write::Whiteout) && child_suffix(dir_key, k).is_some());
        if !has_white {
            return Ok(self
                .store
                .dir_node_at(self.base_root(), dir_comps)
                .await?
                .map(|n| !n.children.is_empty())
                .unwrap_or(false));
        }
        for bp in self.store.list_subtree(self.base_root(), dir_comps).await? {
            let key = path_of(&bp);
            if !matches!(self.upper.get(&key), Some(Write::Whiteout)) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Every currently-live file path strictly under `dir` (base minus whiteouts,
    /// plus upper data). Bounded to the subtree; used by recursive remove/rename.
    async fn live_descendants(
        &self,
        dir_comps: &[String],
        dir_key: &Path,
    ) -> anyhow::Result<BTreeSet<PathBuf>> {
        let mut set: BTreeSet<PathBuf> = BTreeSet::new();
        for bp in self.store.list_subtree(self.base_root(), dir_comps).await? {
            let key = path_of(&bp);
            if !matches!(self.upper.get(&key), Some(Write::Whiteout)) {
                set.insert(key);
            }
        }
        for (k, w) in &self.upper {
            if let Write::Data(_) = w {
                if child_suffix(dir_key, k).is_some() {
                    set.insert(k.clone());
                }
            }
        }
        Ok(set)
    }
}

/// If `path` lives strictly under directory `dir`, return the relative suffix.
/// `dir` empty means the root, under which every path is a child.
fn child_suffix(dir: &Path, path: &Path) -> Option<PathBuf> {
    if dir.as_os_str().is_empty() {
        if path.as_os_str().is_empty() {
            return None;
        }
        return Some(path.to_path_buf());
    }
    path.strip_prefix(dir).ok().and_then(|rest| {
        if rest.as_os_str().is_empty() {
            None // `path == dir`: not a child of itself
        } else {
            Some(rest.to_path_buf())
        }
    })
}
