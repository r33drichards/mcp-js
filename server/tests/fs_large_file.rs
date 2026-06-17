//! Large-file integration: stream a file far bigger than a comfortable
//! in-memory buffer through the incremental writer onto a **disk-backed** store,
//! and verify it round-trips — without the test, the writer, or the store ever
//! holding the whole file in memory at once.
//!
//! The default test runs a 48 MiB file (a couple of seconds). The gigabyte test
//! is `#[ignore]`d; run it with:
//!   cargo test --test fs_large_file -- --ignored --nocapture
//! and optionally size it with `MCP_FS_LARGE_BYTES=<bytes>`.

use server::engine::fs_chunker::{decompress, MAX};
use server::engine::fs_mount::SessionMount;
use server::engine::fs_store::{chunk_key, Content, FileWriter, FsStore};
use server::engine::heap_storage::FileHeapStorage;
use std::path::PathBuf;
use std::sync::Arc;

/// Deterministic, reproducible byte stream — advances one byte per step, so the
/// k-th byte is identical no matter how the stream is sliced into reads/feeds.
struct ByteGen(u64);
impl ByteGen {
    fn new(seed: u64) -> Self {
        Self(seed | 1)
    }
    fn fill(&mut self, buf: &mut [u8]) {
        for b in buf.iter_mut() {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            *b = (x >> 24) as u8;
        }
    }
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcp-fs-large-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Stream `total` bytes into a disk-backed store via `FileWriter`, fed in 1 MiB
/// blocks from `ByteGen`, then verify the stored chunks reproduce the exact
/// stream — fetching and checking one chunk at a time so peak memory stays at a
/// single block plus a single chunk, regardless of `total`.
async fn stream_and_verify(total: usize, seed: u64, tag: &str) {
    let dir = temp_dir(tag);
    let store = FsStore::new(Arc::new(FileHeapStorage::new(&dir)));

        let mut writer = FileWriter::new(store.clone());
    let mut source = ByteGen::new(seed);
    let mut block = vec![0u8; 1024 * 1024];
    let mut remaining = total;
    while remaining > 0 {
        let n = remaining.min(block.len());
        source.fill(&mut block[..n]);
        writer.feed(&block[..n]).await.unwrap();
        remaining -= n;
    }
    let entry = writer.finish().await.unwrap();
    assert_eq!(entry.size as usize, total, "stored size");
    let hashes = match &entry.content {
        Content::Chunks(h) => h.clone(),
        Content::Inline(_) => panic!("a {total}-byte file must be chunked, not inlined"),
    };
    assert!(hashes.len() > 1, "a large file should span many chunks");

        let mut vgen = ByteGen::new(seed);
    let mut expected = vec![0u8; MAX as usize];
    let mut seen = 0usize;
    for h in &hashes {
        let stored = store.blobs().get(&chunk_key(h)).await.unwrap();
        let bytes = decompress(&stored).unwrap();
        assert!(bytes.len() <= MAX as usize, "chunk exceeds max size");
        let exp = &mut expected[..bytes.len()];
        vgen.fill(exp);
        assert_eq!(bytes.as_slice(), &exp[..], "chunk content mismatch at byte {seen}");
        seen += bytes.len();
    }
    assert_eq!(seen, total, "chunks must cover the whole file exactly");

            assert!(store.blobs().list().await.unwrap().len() >= hashes.len());

    let _ = std::fs::remove_dir_all(&dir);
}

"multi_thread"
async fn large_file_streams_to_disk_and_round_trips() {
    stream_and_verify(48 * 1024 * 1024, 0xA5A5_1234, "48m").await;
}

/// A large file written then mounted and re-read through the overlay (push/pull)
/// — the snapshot path end to end, still disk-backed.
"multi_thread"
async fn large_file_pushes_and_pulls_through_a_mount() {
    let dir = temp_dir("mount");
    let store = FsStore::new(Arc::new(FileHeapStorage::new(&dir)));
    let total = 24 * 1024 * 1024;

        let mut writer = FileWriter::new(store.clone());
    let mut source = ByteGen::new(0x77);
    let mut block = vec![0u8; 1024 * 1024];
    let mut remaining = total;
    while remaining > 0 {
        let n = remaining.min(block.len());
        source.fill(&mut block[..n]);
        writer.feed(&block[..n]).await.unwrap();
        remaining -= n;
    }
    let entry = writer.finish().await.unwrap();

    let mut mount = SessionMount::empty(store.clone());
    mount.put_entry("big/file.bin".as_ref(), entry);
    let root = mount.push().await.unwrap();

            let m2 = SessionMount::pull(store.clone(), root).await.unwrap();
    let got = m2.read("big/file.bin".as_ref()).await.unwrap();
    assert_eq!(got.len(), total);
    let mut vgen = ByteGen::new(0x77);
    let mut expected = vec![0u8; total];
    vgen.fill(&mut expected);
    assert_eq!(got, expected, "round-tripped large file mismatch");

    let _ = std::fs::remove_dir_all(&dir);
}

"multi_thread"
"multi-GB/slow; run with --ignored (size via MCP_FS_LARGE_BYTES)"
async fn huge_file_streams_with_bounded_memory() {
    let total: usize = std::env::var("MCP_FS_LARGE_BYTES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1024 * 1024 * 1024);     stream_and_verify(total, 0xC0FFEE, "huge").await;
}
