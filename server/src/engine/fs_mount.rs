//! The userspace overlay mount: a read-only pinned base manifest plus a mutable
//! upper write layer, entirely in-process. No kernel overlayfs/FUSE. `pull`
//! pins a base CA id at mount time so concurrent label moves do not shift a
//! running session; `push` flushes the upper over the base into a new pure
//! manifest and returns its CA id (the durable artifact — the mount is
//! ephemeral).

use crate::engine::fs_store::{Entry, FsStore, Manifest};
use blake3::Hash;
use std::collections::{BTreeSet, HashMap};
use std::path::{Component, Path, PathBuf};

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
    base_id: Option<Hash>,
    base: Manifest, // pinned at pull() time
    upper: HashMap<PathBuf, Write>, // the only mutable state
}

impl SessionMount {
    pub async fn pull(store: FsStore, id: Hash) -> anyhow::Result<Self> {
        let base = store.get_manifest(&id).await?;
        Ok(Self {
            store,
            base_id: Some(id),
            base,
            upper: HashMap::new(),
        })
    }

    pub fn empty(store: FsStore) -> Self {
        Self {
            store,
            base_id: None,
            base: Manifest::default(),
            upper: HashMap::new(),
        }
    }

    pub fn base_id(&self) -> Option<Hash> {
        self.base_id
    }

    /// Resolve the effective entry for a path: upper Data shadows base; a
    /// whiteout (or a missing base entry) means it does not exist.
    fn effective(&self, p: &Path) -> Option<&Entry> {
        match self.upper.get(p) {
            Some(Write::Whiteout) => None,
            Some(Write::Data(e)) => Some(e),
            None => self.base.entries.get(p),
        }
    }

    pub async fn read(&self, p: &Path) -> anyhow::Result<Vec<u8>> {
        let p = normalize(p);
        match self.effective(&p) {
            Some(e) => self.store.read_file(e).await,
            None => anyhow::bail!("ENOENT: {}", p.display()),
        }
    }

    pub async fn write(&mut self, p: &Path, bytes: &[u8]) -> anyhow::Result<()> {
        let entry = self.store.put_file(bytes).await?;
        self.upper.insert(normalize(p), Write::Data(entry));
        Ok(())
    }

    pub async fn unlink(&mut self, p: &Path) -> anyhow::Result<()> {
        let p = normalize(p);
        if self.effective(&p).is_none() {
            anyhow::bail!("ENOENT: {}", p.display());
        }
        self.upper.insert(p, Write::Whiteout);
        Ok(())
    }

    /// Whether `p` names an existing file or (implicit) directory.
    pub fn exists(&self, p: &Path) -> bool {
        let p = normalize(p);
        self.effective(&p).is_some() || self.is_implicit_dir(&p)
    }

    /// Remove a file, or a directory (recursively whiting out descendants).
    /// Mirrors Node `fs.rm`: non-recursive removal of a non-empty directory
    /// fails.
    pub async fn remove(&mut self, p: &Path, recursive: bool) -> anyhow::Result<()> {
        let p = normalize(p);
        if self.effective(&p).is_some() {
            // Plain file.
            self.upper.insert(p, Write::Whiteout);
            return Ok(());
        }
        let descendants: Vec<PathBuf> = self
            .effective_paths()
            .into_iter()
            .filter(|x| child_suffix(&p, x).is_some())
            .collect();
        if descendants.is_empty() {
            anyhow::bail!("ENOENT: {}", p.display());
        }
        if !recursive {
            anyhow::bail!("ENOTEMPTY: {}", p.display());
        }
        for d in descendants {
            self.upper.insert(d, Write::Whiteout);
        }
        Ok(())
    }

    pub async fn stat(&self, p: &Path) -> anyhow::Result<Stat> {
        let p = normalize(p);
        if let Some(e) = self.effective(&p) {
            return Ok(Stat {
                mode: e.mode,
                size: e.size,
                is_dir: false,
                symlink: e.symlink.clone(),
            });
        }
        // Directories are implicit: a path is a directory if anything lives under it.
        if self.is_implicit_dir(&p) {
            return Ok(Stat {
                mode: 0o755,
                size: 0,
                is_dir: true,
                symlink: None,
            });
        }
        anyhow::bail!("ENOENT: {}", p.display())
    }

    /// `mkdir` is a no-op in virtual mode: directories are implied by the paths
    /// of the files they contain. It succeeds so callers can mkdir-then-write.
    pub async fn mkdir(&mut self, _p: &Path) -> anyhow::Result<()> {
        Ok(())
    }

    pub async fn rename(&mut self, from: &Path, to: &Path) -> anyhow::Result<()> {
        let from = normalize(from);
        let to = normalize(to);
        if from == to {
            return Ok(());
        }
        // Plain file: move the single entry.
        if let Some(entry) = self.effective(&from).cloned() {
            self.upper.insert(to, Write::Data(entry));
            self.upper.insert(from, Write::Whiteout);
            return Ok(());
        }
        // Directory (implicit): move every descendant, rewriting its path prefix
        // from `from/...` to `to/...`. Mirrors the host-backed recursive rename.
        if child_suffix(&from, &to).is_some() {
            anyhow::bail!("EINVAL: cannot rename {} into its own subtree", from.display());
        }
        let moves: Vec<(PathBuf, Entry)> = self
            .effective_paths()
            .into_iter()
            .filter_map(|p| {
                let rest = child_suffix(&from, &p)?;
                let entry = self.effective(&p)?.clone();
                Some((rest, entry))
            })
            .collect();
        if moves.is_empty() {
            anyhow::bail!("ENOENT: {}", from.display());
        }
        for (rest, entry) in moves {
            self.upper.insert(to.join(&rest), Write::Data(entry));
            self.upper.insert(from.join(&rest), Write::Whiteout);
        }
        Ok(())
    }

    /// Immediate children (file and subdirectory names) of `dir`, over base ∪
    /// upper minus whiteouts.
    pub async fn readdir(&self, dir: &Path) -> anyhow::Result<Vec<String>> {
        let dir = normalize(dir);
        let mut names: BTreeSet<String> = BTreeSet::new();
        let mut found_any = false;
        for path in self.effective_paths() {
            if let Some(rest) = child_suffix(&dir, &path) {
                found_any = true;
                if let Some(first) = rest.components().next() {
                    names.insert(first.as_os_str().to_string_lossy().to_string());
                }
            }
        }
        if !found_any && !dir.as_os_str().is_empty() {
            // A non-root directory with no descendants does not exist.
            anyhow::bail!("ENOENT: {}", dir.display());
        }
        Ok(names.into_iter().collect())
    }

    /// Flush upper over base into a new pure manifest; return its CA id.
    pub async fn push(&self) -> anyhow::Result<Hash> {
        let mut merged = self.base.clone();
        for (path, w) in &self.upper {
            match w {
                Write::Data(e) => {
                    merged.entries.insert(path.clone(), e.clone());
                }
                Write::Whiteout => {
                    merged.entries.remove(path);
                }
            }
        }
        self.store.put_manifest(&merged).await
    }

    /// All currently-existing file paths (base minus whiteouts, plus upper data).
    fn effective_paths(&self) -> BTreeSet<PathBuf> {
        let mut set: BTreeSet<PathBuf> = BTreeSet::new();
        for k in self.base.entries.keys() {
            if !matches!(self.upper.get(k), Some(Write::Whiteout)) {
                set.insert(k.clone());
            }
        }
        for (k, w) in &self.upper {
            if let Write::Data(_) = w {
                set.insert(k.clone());
            }
        }
        set
    }

    fn is_implicit_dir(&self, dir: &Path) -> bool {
        self.effective_paths()
            .iter()
            .any(|p| child_suffix(dir, p).is_some())
    }
}

/// Normalize a path to the manifest's canonical form: strip a leading `/` and
/// `.` components so `"/a/b"`, `"a/b"`, and `"./a/b"` address the same entry.
fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::Normal(s) => out.push(s),
            Component::CurDir | Component::RootDir | Component::Prefix(_) => {}
            Component::ParentDir => {
                out.pop();
            }
        }
    }
    out
}

/// If `path` lives strictly under directory `dir`, return the relative suffix.
/// `dir` empty means the root, under which every path is a child.
fn child_suffix(dir: &Path, path: &Path) -> Option<PathBuf> {
    if dir.as_os_str().is_empty() {
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
