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

#[derive(Clone, Default)]
struct PrefetchProbe {
    storage: server::engine::heap_storage::MemoryHeapStorage,
    calls: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    slow: Option<String>,
    fail: Option<String>,
    fail_put: Option<String>,
    active: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    peak: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

#[async_trait::async_trait]
impl server::engine::heap_storage::HeapStorage for PrefetchProbe {
    async fn put(&self, name: &str, data: &[u8]) -> Result<(), String> {
        if self.fail_put.as_deref() == Some(name) {
            return Err("primary rejected write".into());
        }
        self.storage.put(name, data).await
    }

    async fn get(&self, name: &str) -> Result<Vec<u8>, String> {
        use std::sync::atomic::Ordering::SeqCst;
        self.calls.lock().unwrap().push(name.into());
        if self.slow.as_deref() == Some(name) {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        }
        if self.fail.as_deref() == Some(name) {
            return Err("required chunk missing".into());
        }
        let active = self.active.fetch_add(1, SeqCst) + 1;
        self.peak.fetch_max(active, SeqCst);
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        self.active.fetch_sub(1, SeqCst);
        self.storage.get(name).await
    }
}

async fn chunk_manifest(store: &FsStore, hashes: Vec<[u8; 32]>) -> [u8; 32] {
    let entries = BTreeMap::from([(
        "file".into(),
        Entry {
            mode: 0o644,
            size: hashes.len() as u64,
            content: Content::Chunks(hashes),
            symlink: None,
        },
    )]);
    *store
        .put_manifest(&Manifest { entries })
        .await
        .unwrap()
        .as_bytes()
}

#[tokio::test]
async fn prefetch_keeps_a_batch_hit_resident_when_a_delayed_miss_arrives() {
    use server::engine::{
        fs_store::chunk_key,
        heap_storage::{HeapStorage, WriteThroughCacheHeapStorage},
    };
    let a = chunk_key(&[1; 32]);
    let b = chunk_key(&[2; 32]);
    let primary = PrefetchProbe::default();
    primary.put(&a, b"a").await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let cache = WriteThroughCacheHeapStorage::with_capacity_bytes(primary, dir.path(), 2);
    let store = FsStore::new(std::sync::Arc::new(cache.clone()));
    let root = chunk_manifest(&store, vec![[1; 32], [2; 32]]).await;
    cache.put(&b, b"b").await.unwrap();
    cache.put("unrelated", b"x").await.unwrap();

    store.prefetch(root).await.unwrap();

    assert!(cache.contains(&a).await.unwrap());
    assert!(cache.contains(&b).await.unwrap());
    assert!(!cache.contains("unrelated").await.unwrap());
}

#[tokio::test]
async fn concurrent_cold_prefetches_share_the_io_limit() {
    use server::engine::{fs_store::chunk_key, heap_storage::HeapStorage};
    use std::sync::atomic::Ordering::SeqCst;
    let probe = PrefetchProbe::default();
    let store = FsStore::new(std::sync::Arc::new(probe.clone()));
    for value in 0..80 {
        probe.put(&chunk_key(&[value; 32]), b"x").await.unwrap();
    }
    let first = chunk_manifest(&store, (0..40).map(|value| [value; 32]).collect()).await;
    let second = chunk_manifest(&store, (40..80).map(|value| [value; 32]).collect()).await;
    let clone = store.clone();
    let (a, b) = tokio::join!(store.prefetch(first), clone.prefetch(second));
    a.unwrap();
    b.unwrap();
    assert_eq!(probe.peak.load(SeqCst), 32);
}

#[tokio::test]
async fn prefetch_reports_a_missing_chunk_without_waiting_for_an_unrelated_read() {
    use server::engine::fs_store::chunk_key;
    let probe = PrefetchProbe {
        slow: Some(chunk_key(&[1; 32])),
        fail: Some(chunk_key(&[2; 32])),
        ..Default::default()
    };
    use server::engine::heap_storage::HeapStorage;
    for value in 3..=40 {
        probe.put(&chunk_key(&[value; 32]), b"x").await.unwrap();
    }
    let store = FsStore::new(std::sync::Arc::new(probe));
    let root = chunk_manifest(&store, (1..=40).map(|value| [value; 32]).collect()).await;
    let error = tokio::time::timeout(std::time::Duration::from_secs(2), store.prefetch(root))
        .await
        .unwrap()
        .unwrap_err();
    assert!(error.to_string().contains("required chunk missing"));
}

#[tokio::test]
async fn prefetch_deduplicates_repeated_chunks_without_flushing_the_batch() {
    use server::engine::{fs_store::chunk_key, heap_storage::HeapStorage};
    use std::sync::atomic::Ordering::SeqCst;
    let probe = PrefetchProbe::default();
    let a = chunk_key(&[1; 32]);
    let b = chunk_key(&[2; 32]);
    probe.put(&a, b"a").await.unwrap();
    probe.put(&b, b"b").await.unwrap();
    let store = FsStore::new(std::sync::Arc::new(probe.clone()));
    let root = chunk_manifest(&store, vec![[1; 32], [1; 32], [1; 32], [2; 32]]).await;

    store.prefetch(root).await.unwrap();

    let mut calls = probe.calls.lock().unwrap().clone();
    calls.sort();
    assert_eq!(calls, vec![a, b]);
    assert_eq!(probe.peak.load(SeqCst), 2);
}

#[tokio::test]
async fn cold_prefetch_processes_ready_directory_chunks_while_a_sibling_is_slow() {
    use server::engine::{fs_store::chunk_key, fs_tree::tree_key};
    let probe = PrefetchProbe::default();
    let writer = FsStore::new(std::sync::Arc::new(probe.clone()));
    let missing = Entry {
        mode: 0o644,
        size: 1,
        content: Content::Chunks(vec![[1; 32]]),
        symlink: None,
    };
    let inline = writer.put_file(b"inline").await.unwrap();
    let entries = BTreeMap::from([("a/file".into(), inline), ("z/file".into(), missing)]);
    let root = *writer.put_manifest(&Manifest { entries }).await.unwrap().as_bytes();
    let slow = writer.get_node(&root).await.unwrap().children["a"].dir.unwrap();
    let probe = PrefetchProbe {
        slow: Some(tree_key(&slow)),
        fail: Some(chunk_key(&[1; 32])),
        ..probe
    };
    let calls = probe.calls.clone();
    let cold = FsStore::new(std::sync::Arc::new(probe));
    let error = tokio::time::timeout(std::time::Duration::from_secs(2), cold.prefetch(root))
        .await.unwrap().unwrap_err();
    assert!(error.to_string().contains("required chunk missing"));
    assert!(calls.lock().unwrap().contains(&tree_key(&slow)));
}

#[tokio::test]
async fn rejected_primary_write_does_not_evict_a_resident_blob() {
    use server::engine::heap_storage::{HeapStorage, WriteThroughCacheHeapStorage};
    let primary = PrefetchProbe {
        fail_put: Some("rejected".into()),
        ..Default::default()
    };
    let dir = tempfile::tempdir().unwrap();
    let cache = WriteThroughCacheHeapStorage::with_capacity_bytes(primary, dir.path(), 1);
    cache.put("resident", b"x").await.unwrap();

    assert_eq!(cache.put("rejected", b"y").await.unwrap_err(), "primary rejected write");
    assert!(cache.contains("resident").await.unwrap());
    assert!(!cache.contains("rejected").await.unwrap());
}
