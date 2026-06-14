//! Content-addressed object store for the snapshottable filesystem.
//!
//! Three object kinds live in the blob backend (shared with the heap store):
//!   * **chunks** — the bytes of large files, keyed by their plaintext hash;
//!   * **tree nodes** — one directory level of a snapshot, keyed by the hash of
//!     its `bincode` encoding (see [`crate::engine::fs_tree`]);
//!   * a snapshot **root** is just the hash of its top-level tree node.
//!
//! All are immutable and idempotent: a content-addressed id always names the
//! same bytes, so they can be written from any node with no coordination.
//!
//! The flat [`Manifest`] type is retained as a convenience view (used by merge,
//! GC roots, and tests): [`FsStore::put_manifest`] builds a tree from it and
//! [`FsStore::get_manifest`] flattens a tree back. The mount itself never
//! materialises a whole `Manifest`; it walks the tree lazily so a multi-TB
//! snapshot costs O(touched paths), not O(total).

use crate::engine::fs_chunker::{chunk_refs, decompress, maybe_compress, SMALL_FILE_MAX, MAX};
use crate::engine::fs_tree::{components_of, path_of, tree_key, TreeChild, TreeNode};
use crate::engine::heap_storage::{HeapStorage, MemoryHeapStorage};
use blake3::Hash;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::Read;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Blob-name prefix keeps chunk blobs from colliding with heap blobs in the
/// shared backend and lets GC recognise what it is scanning.
const CHUNK_PREFIX: &str = "fschunk:";

pub fn chunk_key(hash: &[u8; 32]) -> String {
    format!("{CHUNK_PREFIX}{}", hex(hash))
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Debug)]
pub enum Content {
    /// Tiny files: bytes live directly in the manifest entry.
    Inline(Vec<u8>),
    /// Ordered list of chunk hashes; each chunk is a blob in the store.
    Chunks(Vec<[u8; 32]>),
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Debug)]
pub struct Entry {
    pub mode: u32,
    pub size: u64,
    pub content: Content,
    pub symlink: Option<PathBuf>,
}

/// PURE CONTENT — no parent/lineage field. Identical trees => identical id.
/// Lineage lives only in the pointer plane (labels + reflog). This flat form is
/// a view over the on-disk recursive tree (see module docs).
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Debug, Default)]
pub struct Manifest {
    pub entries: BTreeMap<PathBuf, Entry>,
}

/// Bounded FIFO cache of decoded tree nodes, shared across clones of an
/// `FsStore`. Bounds the resident node set so walking (or building over) a huge
/// tree never pins the whole thing in memory.
struct NodeCache {
    map: HashMap<[u8; 32], Arc<TreeNode>>,
    order: VecDeque<[u8; 32]>,
    cap: usize,
}

impl NodeCache {
    fn new(cap: usize) -> Self {
        Self {
            map: HashMap::new(),
            order: VecDeque::new(),
            cap,
        }
    }

    fn get(&self, id: &[u8; 32]) -> Option<Arc<TreeNode>> {
        self.map.get(id).cloned()
    }

    fn put(&mut self, id: [u8; 32], node: Arc<TreeNode>) {
        if self.map.insert(id, node).is_none() {
            self.order.push_back(id);
            while self.order.len() > self.cap {
                if let Some(old) = self.order.pop_front() {
                    if old != id {
                        self.map.remove(&old);
                    }
                }
            }
        }
    }
}

#[derive(Clone)]
pub struct FsStore {
    blobs: Arc<dyn HeapStorage>,
    nodes: Arc<Mutex<NodeCache>>,
}

impl FsStore {
    /// Reuse the same blob backend the heap store already uses.
    pub fn new(blobs: Arc<dyn HeapStorage>) -> Self {
        Self {
            blobs,
            nodes: Arc::new(Mutex::new(NodeCache::new(8192))),
        }
    }

    /// In-memory backend. Available in normal builds (not just `cfg(test)`) so
    /// integration-test crates, which compile the lib without `--cfg test`, can
    /// construct one.
    pub fn in_memory() -> Self {
        Self::new(Arc::new(MemoryHeapStorage::new()))
    }

    /// Persist file bytes (chunking large files) and return its manifest Entry.
    /// Goes through the same incremental chunker as the streaming paths, so all
    /// write paths produce identical chunk boundaries (consistent dedup).
    pub async fn put_file(&self, data: &[u8]) -> anyhow::Result<Entry> {
        let mut w = FileWriter::new(self.clone());
        w.feed(data).await?;
        w.finish().await
    }

    /// Persist a file from a streaming reader, chunking in bounded memory: the
    /// reader is consumed one block at a time and never fully buffered, so a
    /// multi-GB file is ingested without holding it all at once.
    pub async fn put_file_stream<R: Read>(&self, mut reader: R) -> anyhow::Result<Entry> {
        let mut w = FileWriter::new(self.clone());
        let mut block = vec![0u8; 1024 * 1024];
        loop {
            let n = reader.read(&mut block)?;
            if n == 0 {
                break;
            }
            w.feed(&block[..n]).await?;
        }
        w.finish().await
    }

    pub async fn read_file(&self, entry: &Entry) -> anyhow::Result<Vec<u8>> {
        match &entry.content {
            Content::Inline(b) => Ok(b.clone()),
            Content::Chunks(hashes) => {
                let mut out = Vec::with_capacity(entry.size as usize);
                for h in hashes {
                    let key = chunk_key(h);
                    let stored = self
                        .blobs
                        .get(&key)
                        .await
                        .map_err(|e| anyhow::anyhow!("get chunk: {e}"))?;
                    out.extend_from_slice(&decompress(&stored)?);
                }
                Ok(out)
            }
        }
    }

    // ── Tree nodes ─────────────────────────────────────────────────────────

    /// Fetch and decode a tree node, consulting the shared node cache first.
    pub async fn get_node(&self, id: &[u8; 32]) -> anyhow::Result<Arc<TreeNode>> {
        if let Some(n) = self.nodes.lock().unwrap().get(id) {
            return Ok(n);
        }
        let bytes = self
            .blobs
            .get(&tree_key(id))
            .await
            .map_err(|e| anyhow::anyhow!("get tree node: {e}"))?;
        let node: TreeNode = bincode::deserialize(&bytes)?;
        let arc = Arc::new(node);
        self.nodes.lock().unwrap().put(*id, arc.clone());
        Ok(arc)
    }

    /// Encode and persist a tree node (idempotent); returns its content id.
    pub async fn put_node(&self, node: &TreeNode) -> anyhow::Result<[u8; 32]> {
        let bytes = bincode::serialize(node)?;
        let id = *blake3::hash(&bytes).as_bytes();
        self.blobs
            .put(&tree_key(&id), &bytes)
            .await
            .map_err(|e| anyhow::anyhow!("put tree node: {e}"))?;
        self.nodes.lock().unwrap().put(id, Arc::new(node.clone()));
        Ok(id)
    }

    /// Resolve the child at `comps` under `root`, walking only the nodes on the
    /// path (lazy). The empty path resolves to the root itself (a directory).
    pub async fn resolve(
        &self,
        root: Option<[u8; 32]>,
        comps: &[String],
    ) -> anyhow::Result<Option<TreeChild>> {
        let Some(root) = root else { return Ok(None) };
        if comps.is_empty() {
            return Ok(Some(TreeChild {
                file: None,
                dir: Some(root),
            }));
        }
        let mut node = self.get_node(&root).await?;
        for (i, name) in comps.iter().enumerate() {
            let Some(child) = node.children.get(name) else {
                return Ok(None);
            };
            if i + 1 == comps.len() {
                return Ok(Some(child.clone()));
            }
            let Some(dir) = child.dir else { return Ok(None) };
            node = self.get_node(&dir).await?;
        }
        Ok(None)
    }

    /// The file `Entry` at `comps`, if any.
    pub async fn resolve_file(
        &self,
        root: Option<[u8; 32]>,
        comps: &[String],
    ) -> anyhow::Result<Option<Entry>> {
        Ok(self.resolve(root, comps).await?.and_then(|c| c.file))
    }

    /// The directory node at `comps` (the root node for the empty path), or
    /// `None` if `comps` does not name a directory.
    pub async fn dir_node_at(
        &self,
        root: Option<[u8; 32]>,
        comps: &[String],
    ) -> anyhow::Result<Option<Arc<TreeNode>>> {
        let Some(root) = root else { return Ok(None) };
        if comps.is_empty() {
            return Ok(Some(self.get_node(&root).await?));
        }
        match self.resolve(Some(root), comps).await? {
            Some(TreeChild { dir: Some(d), .. }) => Ok(Some(self.get_node(&d).await?)),
            _ => Ok(None),
        }
    }

    /// Every file path (as component lists) under the subtree at `comps`. Bounded
    /// to that subtree, so it is O(subtree) — used by `remove`/`rename` of a
    /// directory, which inherently touch the whole subtree.
    pub async fn list_subtree(
        &self,
        root: Option<[u8; 32]>,
        comps: &[String],
    ) -> anyhow::Result<Vec<Vec<String>>> {
        let Some(start) = self.dir_node_at(root, comps).await? else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        let mut stack: Vec<(Vec<String>, Arc<TreeNode>)> = vec![(comps.to_vec(), start)];
        while let Some((prefix, node)) = stack.pop() {
            for (name, child) in &node.children {
                let mut p = prefix.clone();
                p.push(name.clone());
                if child.file.is_some() {
                    out.push(p.clone());
                }
                if let Some(d) = child.dir {
                    stack.push((p, self.get_node(&d).await?));
                }
            }
        }
        Ok(out)
    }

    /// Build a new tree from `base_id` with `changes` applied — each `(comps,
    /// Some(entry))` sets a file, each `(comps, None)` clears the file at that
    /// path. Only the nodes on the path-to-root spine of a change are rewritten;
    /// untouched subtrees keep their existing hash (structural sharing). Returns
    /// `None` when the resulting subtree is empty.
    async fn build_node(
        &self,
        base_id: Option<[u8; 32]>,
        changes: Vec<(Vec<String>, Option<Entry>)>,
    ) -> anyhow::Result<Option<[u8; 32]>> {
        let mut node: TreeNode = match base_id {
            Some(id) => (*self.get_node(&id).await?).clone(),
            None => TreeNode::default(),
        };

        // Partition into direct (file at this level) and nested (recurse).
        let mut direct: Vec<(String, Option<Entry>)> = Vec::new();
        let mut nested: BTreeMap<String, Vec<(Vec<String>, Option<Entry>)>> = BTreeMap::new();
        for (mut comps, val) in changes {
            if comps.is_empty() {
                continue;
            }
            if comps.len() == 1 {
                direct.push((comps.pop().unwrap(), val));
            } else {
                let first = comps.remove(0);
                nested.entry(first).or_default().push((comps, val));
            }
        }

        for (name, val) in direct {
            let child = node.children.entry(name.clone()).or_default();
            child.file = val;
            if child.is_empty() {
                node.children.remove(&name);
            }
        }

        for (name, sub) in nested {
            let base_child = node.children.get(&name).and_then(|c| c.dir);
            let new_dir = Box::pin(self.build_node(base_child, sub)).await?;
            let child = node.children.entry(name.clone()).or_default();
            child.dir = new_dir;
            if child.is_empty() {
                node.children.remove(&name);
            }
        }

        if node.children.is_empty() {
            return Ok(None);
        }
        Ok(Some(self.put_node(&node).await?))
    }

    /// Build a snapshot root from `base_id` + `changes`. Always returns a hash
    /// (an empty snapshot becomes the canonical empty-node hash).
    pub async fn build_root(
        &self,
        base_id: Option<[u8; 32]>,
        changes: Vec<(Vec<String>, Option<Entry>)>,
    ) -> anyhow::Result<[u8; 32]> {
        match self.build_node(base_id, changes).await? {
            Some(h) => Ok(h),
            None => self.put_node(&TreeNode::default()).await,
        }
    }

    // ── Flat-manifest view (compat: merge / GC roots / tests) ────────────────

    /// Build a tree from a flat manifest and return its root id.
    pub async fn put_manifest(&self, m: &Manifest) -> anyhow::Result<Hash> {
        let changes: Vec<(Vec<String>, Option<Entry>)> = m
            .entries
            .iter()
            .map(|(p, e)| (components_of(p), Some(e.clone())))
            .collect();
        let root = self.build_root(None, changes).await?;
        Ok(Hash::from_bytes(root))
    }

    /// Flatten the tree rooted at `id` into a manifest. Errors if the root node
    /// is absent (a never-materialised id), matching the old behaviour.
    pub async fn get_manifest(&self, id: &Hash) -> anyhow::Result<Manifest> {
        let root = *id.as_bytes();
        let node = self.get_node(&root).await?;
        let mut entries = BTreeMap::new();
        let mut stack: Vec<(Vec<String>, Arc<TreeNode>)> = vec![(Vec::new(), node)];
        while let Some((prefix, node)) = stack.pop() {
            for (name, child) in &node.children {
                let mut p = prefix.clone();
                p.push(name.clone());
                if let Some(e) = &child.file {
                    entries.insert(path_of(&p), e.clone());
                }
                if let Some(d) = child.dir {
                    stack.push((p, self.get_node(&d).await?));
                }
            }
        }
        Ok(Manifest { entries })
    }

    /// Access to the underlying blob backend (used by GC).
    pub fn blobs(&self) -> &Arc<dyn HeapStorage> {
        &self.blobs
    }
}

/// Once the buffered tail exceeds this, [`FileWriter`] flushes the complete
/// content-defined chunks it has accumulated, keeping only a bounded carry. So
/// the writer's resident memory is ~`FLUSH_THRESHOLD` plus the largest single
/// `feed`, never the whole file.
const FLUSH_THRESHOLD: usize = 2 * MAX as usize;

/// Incremental, push-based file writer: bytes are `feed`-ed in arbitrary pieces
/// and chunked on the fly, so a multi-GB file is stored without ever buffering
/// the whole thing. The chunk boundaries match the one-shot chunker (each
/// emitted chunk ends at a real content-defined cut), so dedup is preserved.
pub struct FileWriter {
    store: FsStore,
    buf: Vec<u8>,        // bytes not yet emitted as a chunk (bounded carry)
    hashes: Vec<[u8; 32]>,
    total: u64,
    chunked: bool, // crossed past the inline threshold
}

impl FileWriter {
    pub fn new(store: FsStore) -> Self {
        Self {
            store,
            buf: Vec::new(),
            hashes: Vec::new(),
            total: 0,
            chunked: false,
        }
    }

    async fn put_chunk(&mut self, hash: &[u8; 32], bytes: &[u8]) -> anyhow::Result<()> {
        let stored = maybe_compress(bytes);
        self.store
            .blobs
            .put(&chunk_key(hash), &stored)
            .await
            .map_err(|e| anyhow::anyhow!("put chunk: {e}"))?;
        self.hashes.push(*hash);
        Ok(())
    }

    /// Append `data`; once enough is buffered, flush the complete chunks and keep
    /// a bounded carry.
    pub async fn feed(&mut self, data: &[u8]) -> anyhow::Result<()> {
        self.total += data.len() as u64;
        self.buf.extend_from_slice(data);
        if self.buf.len() > FLUSH_THRESHOLD {
            self.chunked = true;
            // Emit every complete chunk except the last (which may be a premature
            // end-of-buffer cut); carry the last for the next feed.
            let refs = chunk_refs(&self.buf);
            if refs.len() > 1 {
                let keep_from = refs.last().unwrap().offset;
                let emit: Vec<([u8; 32], usize, usize)> = refs[..refs.len() - 1]
                    .iter()
                    .map(|c| (*c.hash.as_bytes(), c.offset, c.length))
                    .collect();
                for (h, off, len) in emit {
                    let bytes = self.buf[off..off + len].to_vec();
                    self.put_chunk(&h, &bytes).await?;
                }
                self.buf.drain(..keep_from);
            }
        }
        Ok(())
    }

    /// Finish the file: inline if it never grew past the threshold, else flush
    /// the remaining bytes as chunks. Returns the content-addressed entry.
    pub async fn finish(mut self) -> anyhow::Result<Entry> {
        if !self.chunked && self.buf.len() <= SMALL_FILE_MAX {
            return Ok(Entry {
                mode: 0o644,
                size: self.total,
                content: Content::Inline(std::mem::take(&mut self.buf)),
                symlink: None,
            });
        }
        // Flush whatever remains. `chunk_refs` returns empty for a sub-threshold
        // carry, which must still be stored as a (single) chunk.
        let refs = chunk_refs(&self.buf);
        if refs.is_empty() {
            if !self.buf.is_empty() {
                let bytes = std::mem::take(&mut self.buf);
                let h = *blake3::hash(&bytes).as_bytes();
                self.put_chunk(&h, &bytes).await?;
            }
        } else {
            let emit: Vec<([u8; 32], usize, usize)> = refs
                .iter()
                .map(|c| (*c.hash.as_bytes(), c.offset, c.length))
                .collect();
            for (h, off, len) in emit {
                let bytes = self.buf[off..off + len].to_vec();
                self.put_chunk(&h, &bytes).await?;
            }
        }
        Ok(Entry {
            mode: 0o644,
            size: self.total,
            content: Content::Chunks(self.hashes),
            symlink: None,
        })
    }
}
