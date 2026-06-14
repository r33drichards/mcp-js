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
