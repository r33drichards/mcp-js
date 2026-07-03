#![no_main]
use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use server::engine::fs_chunker::{
    chunk_bytes, chunk_refs, decompress, hash_chunk, maybe_compress, Chunked, SMALL_FILE_MAX,
};
use server::engine::fs_store::{FileWriter, FsStore};

#[derive(Arbitrary, Debug)]
enum ChunkerInput {
    /// Arbitrary stored-blob bytes (as if the blob backend were corrupted or
    /// malicious): decoding must error gracefully, never panic.
    DecodeRaw(Vec<u8>),
    /// compress -> decompress must round-trip exactly, for both the zstd and
    /// the raw (incompressible) tag paths.
    CompressRoundTrip(Vec<u8>),
    /// Content-defined chunking invariants: chunks tile the input exactly and
    /// hashes match the plaintext slices.
    ChunkInvariants { data: Vec<u8>, inflate: bool },
    /// Store round-trip: a FileWriter fed the same bytes in arbitrary pieces
    /// must produce the identical entry as the one-shot path, and reading the
    /// entry back must return the original bytes.
    StoreRoundTrip {
        data: Vec<u8>,
        inflate: bool,
        splits: Vec<u16>,
    },
}

/// Cycle `data` past the inline threshold (and usually past the minimum
/// content-defined chunk size) so the chunked storage path runs.
fn inflate_data(data: &[u8]) -> Vec<u8> {
    let pattern: &[u8] = if data.is_empty() { b"\xAB\xCD" } else { data };
    pattern.iter().copied().cycle().take(320_000).collect()
}

fuzz_target!(|input: ChunkerInput| {
    match input {
        ChunkerInput::DecodeRaw(bytes) => {
            // Must not panic; Ok or Err are both acceptable.
            let _ = decompress(&bytes);
        }
        ChunkerInput::CompressRoundTrip(bytes) => {
            let stored = maybe_compress(&bytes);
            let back = decompress(&stored).expect("compressed chunk must decode");
            assert_eq!(back, bytes, "compress/decompress round-trip");
        }
        ChunkerInput::ChunkInvariants { data, inflate } => {
            let data = if inflate { inflate_data(&data) } else { data };
            let refs = chunk_refs(&data);
            if data.len() <= SMALL_FILE_MAX {
                assert!(refs.is_empty(), "small files signal inline via empty refs");
                match chunk_bytes(&data) {
                    Chunked::Inline(b) => assert_eq!(b, data),
                    Chunked::Chunks(_) => panic!("small file must inline"),
                }
            } else {
                // Chunks must tile [0, len) contiguously with matching hashes.
                let mut off = 0;
                for c in &refs {
                    assert_eq!(c.offset, off, "chunks must be contiguous");
                    assert!(c.length > 0, "empty chunk");
                    assert_eq!(
                        c.hash,
                        hash_chunk(&data[c.offset..c.offset + c.length]),
                        "chunk hash must cover its plaintext slice"
                    );
                    off += c.length;
                }
                assert_eq!(off, data.len(), "chunks must cover the whole buffer");
                match chunk_bytes(&data) {
                    Chunked::Chunks(hashes) => assert_eq!(
                        hashes,
                        refs.iter().map(|c| c.hash).collect::<Vec<_>>(),
                        "chunk_bytes and chunk_refs must agree"
                    ),
                    Chunked::Inline(_) => panic!("large file must chunk"),
                }
            }
        }
        ChunkerInput::StoreRoundTrip {
            data,
            inflate,
            splits,
        } => {
            let data = if inflate { inflate_data(&data) } else { data };
            futures::executor::block_on(async {
                let store = FsStore::in_memory();
                let one_shot = store.put_file(&data).await.expect("put_file");
                assert_eq!(one_shot.size, data.len() as u64);
                assert_eq!(
                    store.read_file(&one_shot).await.expect("read_file"),
                    data,
                    "store round-trip must return the original bytes"
                );

                // Feed the same bytes in arbitrary pieces; the resulting entry
                // must be identical (consistent chunk boundaries => dedup).
                let mut writer = FileWriter::new(store.clone());
                let mut pos = 0;
                for s in splits.iter().take(8) {
                    let take = (*s as usize) % (data.len() - pos + 1);
                    writer.feed(&data[pos..pos + take]).await.expect("feed");
                    pos += take;
                }
                writer.feed(&data[pos..]).await.expect("feed tail");
                let split_entry = writer.finish().await.expect("finish");
                assert_eq!(
                    split_entry, one_shot,
                    "split-fed writer must produce the same entry as one-shot"
                );
            });
        }
    }
});
