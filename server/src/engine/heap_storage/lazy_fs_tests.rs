//! Real AWS SDK HTTP requests against a disposable loopback object fixture.
//! No AWS credentials, process environment changes, or live buckets are used.
//! The client belongs to the test runtime; guest ops use separate isolate and
//! synchronous-operation runtimes, exercising S3HeapStorage's existing bridge.

use super::*;
use crate::engine::execution::ExecutionRegistry;
use crate::engine::fs::FsConfig;
use crate::engine::fs_labels::LabelStore;
use crate::engine::fs_mount::SessionMount;
use crate::engine::fs_store::{Content, FsStore, Manifest, chunk_key};
use crate::engine::opa::{EvalMode, PolicyChain};
use crate::engine::{Engine, initialize_v8};
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use std::sync::Once;
use std::time::{Duration, Instant};

#[derive(Default)]
struct Objects {
    data: HashMap<String, Vec<u8>>,
    gets: Vec<String>,
    get_gate: Option<(String, Arc<RequestGate>)>,
    put_gate: Option<Arc<RequestGate>>,
}

#[derive(Default)]
struct RequestGate {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

type FixtureState = Arc<Mutex<Objects>>;

async fn get_object(
    State(state): State<FixtureState>,
    Path((_bucket, key)): Path<(String, String)>,
) -> (StatusCode, Vec<u8>) {
    let (response, gate) = {
        let mut objects = state.lock().unwrap();
        objects.gets.push(key.clone());
        let response = match objects.data.get(&key) {
            Some(bytes) => (StatusCode::OK, bytes.clone()),
            None => (StatusCode::NOT_FOUND, Vec::new()),
        };
        let gate = objects
            .get_gate
            .as_ref()
            .filter(|(blocked, _)| blocked == &key)
            .map(|(_, gate)| gate.clone());
        (response, gate)
    };
    if let Some(gate) = gate {
        gate.entered.notify_one();
        gate.release.notified().await;
    }
    response
}

async fn put_object(
    State(state): State<FixtureState>,
    Path((_bucket, key)): Path<(String, String)>,
    bytes: Bytes,
) -> StatusCode {
    let gate = state.lock().unwrap().put_gate.clone();
    if key.starts_with("fschunk:") {
        if let Some(gate) = gate {
            gate.entered.notify_one();
            gate.release.notified().await;
        }
    }
    state.lock().unwrap().data.insert(key, bytes.to_vec());
    StatusCode::OK
}

struct Fixture {
    state: FixtureState,
    storage: S3HeapStorage,
    server: tokio::task::JoinHandle<()>,
}

impl Fixture {
    async fn new() -> Self {
        let state = Arc::new(Mutex::new(Objects::default()));
        let app = axum::Router::new()
            .route("/{bucket}/{*key}", get(get_object).put(put_object))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let config = aws_sdk_s3::config::Builder::new()
            .behavior_version(aws_config::BehaviorVersion::latest())
            .region(aws_sdk_s3::config::Region::new("us-east-1"))
            .credentials_provider(aws_sdk_s3::config::Credentials::new(
                "fixture", "fixture", None, None, "test",
            ))
            .endpoint_url(format!("http://{address}"))
            .force_path_style(true)
            .retry_config(aws_sdk_s3::config::retry::RetryConfig::disabled())
            .build();
        let storage = S3HeapStorage {
            bucket: "test".into(),
            client: Arc::new(S3Client::from_conf(config)),
            runtime: tokio::runtime::Handle::current(),
        };
        Self {
            state,
            storage,
            server,
        }
    }

    fn take_gets(&self) -> Vec<String> {
        std::mem::take(&mut self.state.lock().unwrap().gets)
    }

    async fn snapshot(&self, files: usize) -> (blake3::Hash, Vec<String>) {
        let store = FsStore::new(Arc::new(self.storage.clone()));
        let mut manifest = Manifest::default();
        let mut chunks = Vec::new();
        for n in 0..files {
            // Above the inline threshold, below one chunk; unique per file.
            let data = vec![b'A' + (n % 26) as u8; 70 * 1024 + n];
            let entry = store.put_file(&data).await.unwrap();
            let Content::Chunks(hashes) = &entry.content else {
                panic!("expected chunk")
            };
            assert_eq!(hashes.len(), 1);
            chunks.push(chunk_key(&hashes[0]));
            manifest
                .entries
                .insert(format!("dir-{n}/data").into(), entry);
        }
        (store.put_manifest(&manifest).await.unwrap(), chunks)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

fn engine(blobs: Arc<dyn HeapStorage>, registry_dir: &std::path::Path) -> Arc<Engine> {
    static INIT: Once = Once::new();
    INIT.call_once(initialize_v8);
    Engine::from_engine(
        Engine::new_stateless(64 * 1024 * 1024, 10, 1)
            .with_fs_config(FsConfig::new(Arc::new(PolicyChain::new(
                vec![],
                EvalMode::All,
            ))))
            .with_execution_registry(Arc::new(
                ExecutionRegistry::new(registry_dir.to_str().unwrap()).unwrap(),
            ))
            .with_fs_snapshots(
                Arc::new(FsStore::new(blobs)),
                Arc::new(LabelStore::in_memory()),
            ),
    )
}

async fn wait_for_run(engine: &Engine, id: String) -> Result<String, String> {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let info = engine
                .get_execution(id.clone())
                .map_err(|e| e.to_string())?;
            match info.status.as_str() {
                "completed" => return info.fs.ok_or_else(|| "missing fs root".to_string()),
                "failed" | "timed_out" | "cancelled" => return Err(format!("{:?}", info.error)),
                _ => tokio::time::sleep(Duration::from_millis(5)).await,
            }
        }
    })
    .await
    .expect("cold filesystem request hung")
}

async fn run(engine: &Engine, root: blake3::Hash, code: &str) -> Result<String, String> {
    let id = engine
        .run_js(code)
        .fs(root.to_hex().to_string())
        .execute()
        .await?;
    wait_for_run(engine, id).await
}

// A current-thread owner runtime is deliberate: the JS worker must not block
// it while waiting for cold S3 I/O from either async or sync guest operations.
#[tokio::test]
async fn lazy_fs_cold_mount_cost_does_not_scale_with_unread_chunks() {
    for files in [4, 128] {
        let fixture = Fixture::new().await;
        let (root, _) = fixture.snapshot(files).await;

        // Reproduce the old mount's request cost using the same actual backend.
        let eager = FsStore::new(Arc::new(fixture.storage.clone()));
        fixture.take_gets();
        let start = Instant::now();
        eager.prefetch(*root.as_bytes()).await.unwrap();
        let eager_elapsed = start.elapsed();
        let eager_gets = fixture.take_gets();
        assert_eq!(eager_gets.len(), 2 * files + 1);

        let dir = tempfile::tempdir().unwrap();
        let engine = engine(Arc::new(fixture.storage.clone()), dir.path());
        let start = Instant::now();
        assert_eq!(
            run(&engine, root, "1 + 1").await.unwrap(),
            root.to_hex().to_string()
        );
        let lazy_elapsed = start.elapsed();
        let lazy_gets = fixture.take_gets();
        assert_eq!(lazy_gets.len(), 1, "mount must validate only the root");
        assert!(lazy_gets[0].starts_with("fstree:"));
        eprintln!(
            "files={files}: eager mount={} GETs ({eager_elapsed:?}); lazy whole run={} GETs ({lazy_elapsed:?})",
            eager_gets.len(),
            lazy_gets.len()
        );

        run(&engine, root, r#"
            const st = await fs.stat('/dir-0/data');
            if (st.size !== 71680) throw new Error('stat mismatch');
            if ((await fs.readdir('/dir-0')).join(',') !== 'data') throw new Error('readdir mismatch');
        "#).await.unwrap();
        let gets = fixture.take_gets();
        assert_eq!(
            gets.len(),
            1,
            "metadata must fetch only the selected directory"
        );
        assert!(gets[0].starts_with("fstree:"));
        engine.shutdown().await;
    }
}

#[tokio::test]
async fn lazy_fs_cold_async_sync_reads_and_writes_survive_cache_eviction() {
    // Plain S3, a one-object cache (forces eviction), and a warm working set.
    for capacity in [None, Some(1), Some(4 * 1024 * 1024)] {
        let fixture = Fixture::new().await;
        let (root, chunks) = fixture.snapshot(16).await;
        let dir = tempfile::tempdir().unwrap();
        let cache_dir = tempfile::tempdir().unwrap();
        let blobs: Arc<dyn HeapStorage> = match capacity {
            None => Arc::new(fixture.storage.clone()),
            Some(cap) => Arc::new(WriteThroughCacheHeapStorage::with_capacity_bytes(
                fixture.storage.clone(),
                cache_dir.path(),
                cap,
            )),
        };
        let engine = engine(blobs, dir.path());
        fixture.take_gets();
        run(
            &engine,
            root,
            r#"
            const a = await fs.readFile('/dir-0/data', 'utf8');
            const b = fs.readFileSync('/dir-1/data', 'utf8');
            const again = await fs.readFile('/dir-0/data', 'utf8');
            if (a !== 'A'.repeat(71680) || b !== 'B'.repeat(71681) || again !== a)
                throw new Error('cold read mismatch');
        "#,
        )
        .await
        .unwrap();
        let gets = fixture.take_gets();
        let chunk_gets: Vec<_> = gets
            .iter()
            .filter(|key| key.starts_with("fschunk:"))
            .collect();
        assert!(
            chunk_gets
                .iter()
                .all(|key| **key == chunks[0] || **key == chunks[1])
        );
        assert_eq!(
            chunk_gets.len(),
            if capacity == Some(4 * 1024 * 1024) {
                2
            } else {
                3
            }
        );

        let next = run(
            &engine,
            root,
            r#"
            await fs.writeFile('/new/async', 'C'.repeat(71682));
            fs.writeFileSync('/new/sync', 'D'.repeat(71683));
        "#,
        )
        .await
        .unwrap();
        assert!(
            fixture
                .take_gets()
                .iter()
                .all(|key| !key.starts_with("fschunk:")),
            "writing new files must not read old file contents"
        );
        let reopened = SessionMount::pull(
            FsStore::new(Arc::new(fixture.storage.clone())),
            blake3::Hash::from_hex(&next).unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(
            reopened.read("new/async".as_ref()).await.unwrap(),
            vec![b'C'; 71682]
        );
        assert_eq!(
            reopened.read("new/sync".as_ref()).await.unwrap(),
            vec![b'D'; 71683]
        );
        assert_eq!(
            reopened.read("dir-0/data".as_ref()).await.unwrap(),
            vec![b'A'; 71680]
        );
        engine.shutdown().await;
    }
}

#[tokio::test]
async fn lazy_fs_missing_blobs_fail_when_accessed_and_invalid_roots_still_fail_mount() {
    let fixture = Fixture::new().await;
    let (root, chunks) = fixture.snapshot(4).await;
    fixture.state.lock().unwrap().data.remove(&chunks[3]);
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(Arc::new(fixture.storage.clone()), dir.path());
    // Unrelated unavailable contents no longer prevent useful work.
    run(&engine, root, "await fs.readFile('/dir-0/data')")
        .await
        .unwrap();
    let error = run(&engine, root, "await fs.readFile('/dir-3/data')")
        .await
        .unwrap_err();
    assert!(error.contains("get chunk"), "{error}");
    let error = run(&engine, root, "await fs.appendFile('/dir-3/data', 'tail')")
        .await
        .unwrap_err();
    assert!(
        error.contains("get chunk"),
        "append must not replace unreadable contents: {error}"
    );

    // Missing descendant metadata must not be mistaken for a free symlink
    // destination. Use a fresh decoded cache after removing the directory.
    let store = FsStore::new(Arc::new(fixture.storage.clone()));
    let node = store.get_node(root.as_bytes()).await.unwrap();
    let subtree = node.children["dir-2"].dir.unwrap();
    fixture
        .state
        .lock()
        .unwrap()
        .data
        .remove(&crate::engine::fs_tree::tree_key(&subtree));
    let other_dir = tempfile::tempdir().unwrap();
    let fresh_engine = self::engine(Arc::new(fixture.storage.clone()), other_dir.path());
    run(
        &fresh_engine,
        root,
        r#"
        let rejected = false;
        try { await fs.symlink('/target', '/dir-2/link'); }
        catch (e) { rejected = String(e).includes('get tree node'); }
        if (!rejected) throw new Error('storage failure was treated as absence');
    "#,
    )
    .await
    .unwrap();
    fresh_engine.shutdown().await;

    let invalid = blake3::hash(b"nonexistent root");
    let error = run(&engine, invalid, "throw new Error('must not run')")
        .await
        .unwrap_err();
    assert!(error.contains("fs mount: root"), "{error}");
    engine.shutdown().await;
}

#[tokio::test]
async fn lazy_fs_pending_async_io_does_not_deadlock_synchronous_guest_calls() {
    for (writing, queued) in [
        (false, "fs.readFile('/dir-2/data')"),
        (true, "fs.readFile('/dir-2/data')"),
        (false, "fs.createWriteStream('/stream-open')"),
        (false, "writer.close()"),
    ] {
        let fixture = Fixture::new().await;
        let (root, chunks) = fixture.snapshot(4).await;
        let gate = Arc::new(RequestGate::default());
        if writing {
            fixture.state.lock().unwrap().put_gate = Some(gate.clone());
        } else {
            fixture.state.lock().unwrap().get_gate = Some((chunks[0].clone(), gate.clone()));
        }
        let dir = tempfile::tempdir().unwrap();
        let engine = engine(Arc::new(fixture.storage.clone()), dir.path());
        let operation = if writing {
            "fs.writeFile('/pending', 'X'.repeat(71680))"
        } else {
            "fs.readFile('/dir-0/data', 'utf8')"
        };
        let code = format!(
            r#"
            const writer = await fs.createWriteStream('/stream-close');
            await writer.write('streamed contents');
            const pending = {operation};
            await new Promise(resolve => setTimeout(resolve, 25));
            const queued = {queued};
            await new Promise(resolve => setTimeout(resolve, 25));
            console.log('sync-start' + '.'.repeat(4096));
            const b = fs.readFileSync('/dir-1/data', 'utf8');
            if (b !== 'B'.repeat(71681)) throw new Error('sync read mismatch');
            await pending;
            await queued;
        "#
        );
        let id = engine
            .run_js(code)
            .fs(root.to_hex().to_string())
            .execute()
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), gate.entered.notified())
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let output = engine
                    .get_execution_output(id.clone(), None, None, None, None)
                    .unwrap();
                if output.data.contains("sync-start") {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        gate.release.notify_one();
        wait_for_run(&engine, id).await.unwrap();
        engine.shutdown().await;
    }
}

#[tokio::test]
async fn lazy_fs_cancelled_write_does_not_publish_a_snapshot() {
    let fixture = Fixture::new().await;
    let (root, _) = fixture.snapshot(4).await;
    let gate = Arc::new(RequestGate::default());
    fixture.state.lock().unwrap().put_gate = Some(gate.clone());
    let dir = tempfile::tempdir().unwrap();
    let engine = engine(Arc::new(fixture.storage.clone()), dir.path());
    let id = engine
        .run_js("await fs.writeFile('/pending', 'X'.repeat(71680))")
        .fs(root.to_hex().to_string())
        .execute()
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), gate.entered.notified())
        .await
        .unwrap();
    engine.cancel_execution(id.clone()).unwrap();
    // V8 termination does not itself wake a Rust network future. Release the
    // dispatched PUT, then require the execution and its service tasks to stop.
    gate.release.notify_one();
    let registry = engine.execution_registry.as_ref().unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !registry.is_stopped(&id) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("cancelled execution must stop after pending I/O wakes");
    assert!(engine.get_execution(id).unwrap().fs.is_none());
    // The immutable S3 PUT may have finished, but no snapshot was published.
    // The separate retained-mount unit test asserts no late upper mutation.
    let mount = SessionMount::pull(FsStore::new(Arc::new(fixture.storage.clone())), root)
        .await
        .unwrap();
    assert!(mount.read("pending".as_ref()).await.is_err());
    engine.shutdown().await;
}
