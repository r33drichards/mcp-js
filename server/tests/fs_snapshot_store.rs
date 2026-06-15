use server::engine::fs_chunker::{chunk_bytes, Chunked, SMALL_FILE_MAX};

/// Deterministic high-entropy bytes (xorshift64) so FastCDC finds real content
/// boundaries — a periodic pattern would only ever hit the MAX-size cap.
fn pseudo_random(len: usize, seed: u64) -> Vec<u8> {
    let mut state = seed | 1;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 24) as u8
        })
        .collect()
}

#[test]
fn small_file_is_stored_inline_not_chunked() {
    let data = b"hello world";
    match chunk_bytes(data) {
        Chunked::Inline(bytes) => assert_eq!(bytes, data),
        _ => panic!("small file should be inlined"),
    }
}

#[test]
fn boundary_file_at_threshold_is_inlined() {
    let data = vec![7u8; SMALL_FILE_MAX];
    assert!(matches!(chunk_bytes(&data), Chunked::Inline(_)));
}

#[test]
fn large_file_chunks_are_deterministic() {
    let data = pseudo_random(4 * 1024 * 1024, 0x31);
    let a = chunk_bytes(&data);
    let b = chunk_bytes(&data);
    assert_eq!(a, b, "chunking must be deterministic");
    assert!(matches!(a, Chunked::Chunks(_)));
}

#[test]
fn single_byte_edit_changes_few_chunks() {
    let mut data = pseudo_random(8 * 1024 * 1024, 0x17);
    let before = match chunk_bytes(&data) {
        Chunked::Chunks(c) => c,
        _ => panic!(),
    };
    data[4 * 1024 * 1024] ^= 0xFF; // flip one byte in the middle
    let after = match chunk_bytes(&data) {
        Chunked::Chunks(c) => c,
        _ => panic!(),
    };
    let shared = before.iter().filter(|h| after.contains(h)).count();
    assert!(
        shared >= before.len() - 3,
        "a 1-byte edit should re-store at most a few chunks, shared={shared} of {}",
        before.len()
    );
}

use server::engine::fs_store::{Content, Entry, FsStore, Manifest};
use std::collections::BTreeMap;

#[tokio::test]
async fn identical_trees_produce_identical_manifest_ids() {
    let store = FsStore::in_memory();
    let mut a = Manifest {
        entries: BTreeMap::new(),
    };
    a.entries
        .insert("a.txt".into(), store.put_file(b"x").await.unwrap());
    a.entries
        .insert("b.txt".into(), store.put_file(b"y").await.unwrap());

    let mut b = Manifest {
        entries: BTreeMap::new(),
    };
    b.entries
        .insert("b.txt".into(), store.put_file(b"y").await.unwrap());
    b.entries
        .insert("a.txt".into(), store.put_file(b"x").await.unwrap());

    assert_eq!(
        store.put_manifest(&a).await.unwrap(),
        store.put_manifest(&b).await.unwrap(),
        "insertion order must not affect the manifest id"
    );
}

#[tokio::test]
async fn manifest_round_trips_and_reads_file_content() {
    let store = FsStore::in_memory();
    let mut m = Manifest {
        entries: BTreeMap::new(),
    };
    m.entries
        .insert("dir/f".into(), store.put_file(b"payload").await.unwrap());
    let id = store.put_manifest(&m).await.unwrap();

    let got = store.get_manifest(&id).await.unwrap();
    let entry = got.entries.get(std::path::Path::new("dir/f")).unwrap();
    assert_eq!(store.read_file(entry).await.unwrap(), b"payload");
}

#[tokio::test]
async fn large_file_round_trips_through_chunk_store() {
    let store = FsStore::in_memory();
    let data: Vec<u8> = {
        let mut s = 0x99u64 | 1;
        (0..(3 * 1024 * 1024))
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                (s >> 24) as u8
            })
            .collect()
    };
    let entry = store.put_file(&data).await.unwrap();
    assert!(matches!(entry.content, Content::Chunks(_)));
    assert_eq!(entry.size, data.len() as u64);
    assert_eq!(store.read_file(&entry).await.unwrap(), data);
}

#[tokio::test]
async fn tiny_file_is_inline_entry() {
    let store = FsStore::in_memory();
    let entry: Entry = store.put_file(b"small").await.unwrap();
    assert!(matches!(entry.content, Content::Inline(_)));
}
