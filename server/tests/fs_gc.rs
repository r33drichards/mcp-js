//! Reflog-rooted garbage collection for the fs object store.

use server::engine::fs_gc::{collect, ReflogRetention};
use server::engine::fs_labels::LabelStore;
use server::engine::fs_mount::SessionMount;
use server::engine::fs_store::FsStore;
use server::engine::fs_tree::tree_key;

/// Push `files` as a snapshot via a mount and return its CA id (32 bytes).
async fn snapshot(store: &FsStore, files: &[(&str, &[u8])]) -> [u8; 32] {
    let mut mnt = SessionMount::empty(store.clone());
    for (p, data) in files {
        mnt.write(p.as_ref(), data).await.unwrap();
    }
    *mnt.push().await.unwrap().as_bytes()
}

/// Whether the snapshot's root tree node is still present in the backend.
async fn manifest_exists(store: &FsStore, id: &[u8; 32]) -> bool {
    store.blobs().get(&tree_key(id)).await.is_ok()
}


async fn gc_keeps_reachable_and_sweeps_unreferenced() {
    let store = FsStore::in_memory();
    let labels = LabelStore::in_memory();

    let live = snapshot(&store, &[("a.txt", b"live")]).await;
    let garbage = snapshot(&store, &[("b.txt", b"orphan never labelled")]).await;
    labels.create("main", live, None).await.unwrap();

    let stats = collect(&store, &labels, ReflogRetention::KeepAll)
        .await
        .unwrap();
    assert!(stats.deleted >= 1, "the orphan manifest should be swept");
    assert!(manifest_exists(&store, &live).await, "labelled snapshot kept");
    assert!(
        !manifest_exists(&store, &garbage).await,
        "unreferenced snapshot collected"
    );
}


async fn reset_then_gc_does_not_collect_rolled_past_snapshot() {
    let store = FsStore::in_memory();
    let labels = LabelStore::in_memory();

    let v1 = snapshot(&store, &[("f", b"v1")]).await;
    let v2 = snapshot(&store, &[("f", b"v2")]).await;
    labels.create("main", v1, None).await.unwrap();
    labels.cas("main", Some(v1), v2, None).await.unwrap();     labels.force("main", v1, None).await.unwrap(); 
        let stats = collect(&store, &labels, ReflogRetention::KeepAll)
        .await
        .unwrap();
    assert!(
        manifest_exists(&store, &v2).await,
        "rolled-past snapshot must remain while it is in the reflog"
    );
    assert!(manifest_exists(&store, &v1).await, "current head kept");
    assert_eq!(stats.deleted, 0);

            collect(&store, &labels, ReflogRetention::KeepLast(1))
        .await
        .unwrap();
    assert!(
        !manifest_exists(&store, &v2).await,
        "after reflog pruning the rolled-past snapshot is collectable"
    );
    assert!(manifest_exists(&store, &v1).await, "head still reachable");
}


async fn gc_preserves_shared_chunks_across_snapshots() {
    let store = FsStore::in_memory();
    let labels = LabelStore::in_memory();

        let big: Vec<u8> = {
        let mut s = 0x1234u64 | 1;
        (0..(2 * 1024 * 1024))
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                (s >> 24) as u8
            })
            .collect()
    };
    let shared = snapshot(&store, &[("big.bin", &big)]).await;
    let also = snapshot(&store, &[("big.bin", &big), ("note", b"x")]).await;
    labels.create("a", shared, None).await.unwrap();
    labels.create("b", also, None).await.unwrap();

    collect(&store, &labels, ReflogRetention::KeepAll)
        .await
        .unwrap();

        let m = SessionMount::pull(store.clone(), blake3::Hash::from_bytes(shared))
        .await
        .unwrap();
    assert_eq!(m.read("big.bin".as_ref()).await.unwrap(), big);
}
