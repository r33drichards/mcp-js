//! Content-addressed object store for the snapshottable filesystem.
//!
//! Two object kinds live in the blob backend (shared with the heap store):
//!   * **chunks** — the bytes of large files, keyed by their plaintext hash;
//!   * **manifests** — pure-content directory trees, keyed by the hash of their
//!     canonical (`bincode` over a `BTreeMap`) encoding.
//!
//! Both are immutable and idempotent: a content-addressed id always names the
//! same bytes, so they can be written from any node with no coordination.

use crate::engine::fs_chunker::{chunk_refs, decompress, maybe_compress, Chunked};
use crate::engine::heap_storage::{HeapStorage, MemoryHeapStorage};
use blake3::Hash;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

/// Blob-name prefixes keep fs objects from colliding with heap blobs in the
/// shared backend and let GC recognise what it is scanning.
const CHUNK_PREFIX: &str = "fschunk:";
const MANIFEST_PREFIX: &str = "fsmanifest:";

pub fn chunk_key(hash: &[u8; 32]) -> String {
    format!("{CHUNK_PREFIX}{}", hex(hash))
}

pub fn manifest_key(hash: &[u8; 32]) -> String {
    format!("{MANIFEST_PREFIX}{}", hex(hash))
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
/// Lineage lives only in the pointer plane (labels + reflog).
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Debug, Default)]
pub struct Manifest {
    pub entries: BTreeMap<PathBuf, Entry>,
}

#[derive(Clone)]
pub struct FsStore {
    blobs: Arc<dyn HeapStorage>,
}

impl FsStore {
    /// Reuse the same blob backend the heap store already uses.
    pub fn new(blobs: Arc<dyn HeapStorage>) -> Self {
        Self { blobs }
    }

    /// In-memory backend. Available in normal builds (not just `cfg(test)`) so
    /// integration-test crates, which compile the lib without `--cfg test`, can
    /// construct one.
    pub fn in_memory() -> Self {
        Self::new(Arc::new(MemoryHeapStorage::new()))
    }

    /// Persist file bytes (chunking large files) and return its manifest Entry.
    /// Chunks and persists in a single pass over `data`.
    pub async fn put_file(&self, data: &[u8]) -> anyhow::Result<Entry> {
        let refs = chunk_refs(data);
        let content = if refs.is_empty() {
            // Tiny file: inline, no blob round-trip. (`chunk_refs` returns empty
            // for files at or below the inline threshold.)
            debug_assert!(matches!(
                crate::engine::fs_chunker::chunk_bytes(data),
                Chunked::Inline(_)
            ));
            Content::Inline(data.to_vec())
        } else {
            let mut out = Vec::with_capacity(refs.len());
            for c in refs {
                let raw = &data[c.offset..c.offset + c.length];
                let key = chunk_key(c.hash.as_bytes());
                let stored = maybe_compress(raw);
                self.blobs
                    .put(&key, &stored)
                    .await
                    .map_err(|e| anyhow::anyhow!("put chunk: {e}"))?;
                out.push(*c.hash.as_bytes());
            }
            Content::Chunks(out)
        };
        Ok(Entry {
            mode: 0o644,
            size: data.len() as u64,
            content,
            symlink: None,
        })
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

    pub async fn put_manifest(&self, m: &Manifest) -> anyhow::Result<Hash> {
        let bytes = bincode::serialize(m)?; // deterministic
        let id = blake3::hash(&bytes);
        self.blobs
            .put(&manifest_key(id.as_bytes()), &bytes)
            .await
            .map_err(|e| anyhow::anyhow!("put manifest: {e}"))?; // idempotent
        Ok(id)
    }

    pub async fn get_manifest(&self, id: &Hash) -> anyhow::Result<Manifest> {
        let bytes = self
            .blobs
            .get(&manifest_key(id.as_bytes()))
            .await
            .map_err(|e| anyhow::anyhow!("get manifest: {e}"))?;
        Ok(bincode::deserialize(&bytes)?)
    }

    /// Access to the underlying blob backend (used by GC).
    pub fn blobs(&self) -> &Arc<dyn HeapStorage> {
        &self.blobs
    }
}
