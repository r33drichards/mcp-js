//! Content-defined chunking with hybrid whole/chunked storage.
//!
//! Tiny files are inlined directly into the manifest entry (no blob round-trip).
//! Large files are split into content-defined chunks via FastCDC; each chunk is
//! hashed by its *plaintext* bytes so dedup is by content, and stored
//! (optionally zstd-compressed) as a blob keyed by that hash.

use blake3::Hash;
use fastcdc::v2020::FastCDC;

/// Files at or below this size are stored whole (and inlined into the manifest).
pub const SMALL_FILE_MAX: usize = 64 * 1024;

/// FastCDC content-defined chunking parameters (min/avg/max chunk size). Shared
/// by the slice chunker here and the streaming chunker in `fs_store`.
pub const MIN: u32 = 256 * 1024;
pub const AVG: u32 = 1024 * 1024;
pub const MAX: u32 = 4 * 1024 * 1024;


pub enum Chunked {
    /// Inlined directly in the manifest entry (tiny files, no blob round-trip).
    Inline(Vec<u8>),
    /// Ordered list of chunk hashes; each chunk is a blob in the store.
    Chunks(Vec<Hash>),
}

/// A single content-defined chunk: its plaintext hash plus the byte range
/// `[offset, offset+length)` within the original buffer. Lets callers persist
/// chunk bytes in a single pass over the data without re-slicing.

pub struct ChunkRef {
    pub hash: Hash,
    pub offset: usize,
    pub length: usize,
}

/// Hash a chunk's *uncompressed* bytes so dedup is by plaintext content.
pub fn hash_chunk(bytes: &[u8]) -> Hash {
    blake3::hash(bytes)
}

/// Split file bytes into content-defined chunks (or inline if tiny).
/// Returns chunk hashes only; callers persist the chunk bytes via FsStore.
pub fn chunk_bytes(data: &[u8]) -> Chunked {
    if data.len() <= SMALL_FILE_MAX {
        return Chunked::Inline(data.to_vec());
    }
    Chunked::Chunks(chunk_refs(data).into_iter().map(|c| c.hash).collect())
}

/// Compute content-defined chunk boundaries and hashes in a single pass.
/// For files at or below [`SMALL_FILE_MAX`] this returns an empty vec, signalling
/// the caller should inline instead.
pub fn chunk_refs(data: &[u8]) -> Vec<ChunkRef> {
    if data.len() <= SMALL_FILE_MAX {
        return Vec::new();
    }
    FastCDC::new(data, MIN, AVG, MAX)
        .map(|c| ChunkRef {
            hash: hash_chunk(&data[c.offset..c.offset + c.length]),
            offset: c.offset,
            length: c.length,
        })
        .collect()
}

/// One-byte tag prefixed to stored chunk blobs so reads know how to decode.
const TAG_RAW: u8 = 0;
const TAG_ZSTD: u8 = 1;

/// zstd-compress a chunk, keeping the result only if it actually shrinks.
/// The returned bytes are tagged so [`decompress`] can round-trip them.
pub fn maybe_compress(chunk: &[u8]) -> Vec<u8> {
    match zstd::encode_all(chunk, 3) {
        Ok(z) if z.len() < chunk.len() => {
            let mut out = Vec::with_capacity(z.len() + 1);
            out.push(TAG_ZSTD);
            out.extend_from_slice(&z);
            out
        }
        _ => {
            let mut out = Vec::with_capacity(chunk.len() + 1);
            out.push(TAG_RAW);
            out.extend_from_slice(chunk);
            out
        }
    }
}

/// Inverse of [`maybe_compress`]: strip the tag and decode if needed.
pub fn decompress(stored: &[u8]) -> anyhow::Result<Vec<u8>> {
    match stored.split_first() {
        Some((&TAG_RAW, rest)) => Ok(rest.to_vec()),
        Some((&TAG_ZSTD, rest)) => {
            Ok(zstd::decode_all(rest).map_err(|e| anyhow::anyhow!("zstd decode: {e}"))?)
        }
        _ => anyhow::bail!("corrupt chunk blob: missing compression tag"),
    }
}
