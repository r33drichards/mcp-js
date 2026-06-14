use server::engine::fs_store::{FsStore, Manifest};
use server::engine::fs_mount::SessionMount;
use std::path::PathBuf;

/// Build a base manifest from `(path, bytes)` pairs and return the store + id.
async fn base_with(files: &[(&str, &[u8])]) -> (FsStore, blake3::Hash) {
    let store = FsStore::in_memory();
    let mut m = Manifest::default();
    for (p, data) in files {
        m.entries
            .insert(PathBuf::from(p), store.put_file(data).await.unwrap());
    }
    let id = store.put_manifest(&m).await.unwrap();
    (store, id)
}

#[tokio::test]
async fn upper_shadows_base_and_whiteout_hides_base() {
    let (store, base) = base_with(&[("a.txt", b"base")]).await;
    let mut mnt = SessionMount::pull(store.clone(), base).await.unwrap();

    assert_eq!(mnt.read("a.txt".as_ref()).await.unwrap(), b"base");
    mnt.write("a.txt".as_ref(), b"override").await.unwrap();
    assert_eq!(mnt.read("a.txt".as_ref()).await.unwrap(), b"override"); // upper wins
    mnt.unlink("a.txt".as_ref()).await.unwrap();
    assert!(mnt.read("a.txt".as_ref()).await.is_err()); // whiteout hides base
}

#[tokio::test]
async fn push_is_pure_and_dedups() {
    let (store, base) = base_with(&[("a.txt", b"base")]).await;
    let mut a = SessionMount::pull(store.clone(), base).await.unwrap();
    a.write("new.txt".as_ref(), b"data").await.unwrap();
    let id_a = a.push().await.unwrap();

    let mut b = SessionMount::pull(store.clone(), base).await.unwrap();
    b.write("new.txt".as_ref(), b"data").await.unwrap();
    let id_b = b.push().await.unwrap();

    assert_eq!(id_a, id_b, "same resulting tree => same CA id");
}

#[tokio::test]
async fn push_then_pull_round_trips_new_and_modified_files() {
    let (store, base) = base_with(&[("a.txt", b"base"), ("keep.txt", b"keep")]).await;
    let mut m = SessionMount::pull(store.clone(), base).await.unwrap();
    m.write("a.txt".as_ref(), b"changed").await.unwrap();
    m.write("nested/dir/n.txt".as_ref(), b"new").await.unwrap();
    let id = m.push().await.unwrap();

    let reread = SessionMount::pull(store.clone(), id).await.unwrap();
    assert_eq!(reread.read("a.txt".as_ref()).await.unwrap(), b"changed");
    assert_eq!(reread.read("keep.txt".as_ref()).await.unwrap(), b"keep");
    assert_eq!(
        reread.read("nested/dir/n.txt".as_ref()).await.unwrap(),
        b"new"
    );
}

#[tokio::test]
async fn whiteout_drops_file_from_pushed_manifest() {
    let (store, base) = base_with(&[("gone.txt", b"x"), ("stay.txt", b"y")]).await;
    let mut m = SessionMount::pull(store.clone(), base).await.unwrap();
    m.unlink("gone.txt".as_ref()).await.unwrap();
    let id = m.push().await.unwrap();

    let manifest = store.get_manifest(&id).await.unwrap();
    assert!(!manifest.entries.contains_key(std::path::Path::new("gone.txt")));
    assert!(manifest.entries.contains_key(std::path::Path::new("stay.txt")));
}

#[tokio::test]
async fn empty_mount_starts_blank_and_accepts_writes() {
    let store = FsStore::in_memory();
    let mut m = SessionMount::empty(store.clone());
    assert!(m.base_id().is_none());
    assert!(m.read("x".as_ref()).await.is_err());
    m.write("x".as_ref(), b"hi").await.unwrap();
    assert_eq!(m.read("x".as_ref()).await.unwrap(), b"hi");
}

#[tokio::test]
async fn readdir_lists_children_minus_whiteouts() {
    let (store, base) = base_with(&[("d/a", b"1"), ("d/b", b"2"), ("d/sub/c", b"3")]).await;
    let mut m = SessionMount::pull(store.clone(), base).await.unwrap();
    m.unlink("d/a".as_ref()).await.unwrap();
    m.write("d/new".as_ref(), b"4").await.unwrap();

    let mut kids = m.readdir("d".as_ref()).await.unwrap();
    kids.sort();
    assert_eq!(kids, vec!["b", "new", "sub"]);
}

#[tokio::test]
async fn rename_moves_content_and_whiteouts_source() {
    let (store, base) = base_with(&[("from.txt", b"data")]).await;
    let mut m = SessionMount::pull(store.clone(), base).await.unwrap();
    m.rename("from.txt".as_ref(), "to.txt".as_ref())
        .await
        .unwrap();
    assert!(m.read("from.txt".as_ref()).await.is_err());
    assert_eq!(m.read("to.txt".as_ref()).await.unwrap(), b"data");
}
