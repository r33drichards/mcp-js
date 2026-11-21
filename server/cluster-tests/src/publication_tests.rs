//! Deterministic metadata/publication tests against the real consensus handlers.
use crate::{cluster::*, raft_metadata::RaftMetadataStore};
use mcp_metadata_store::{
    MetadataStore, Write,
    publication::{MetadataPublication, Publication, PublishOutcome, ReadMode},
};
use std::{collections::HashMap, sync::Arc, time::Duration};

fn node(id: &str) -> Arc<ClusterNode> {
    ClusterNode::new(
        ClusterConfig {
            node_id: id.into(),
            peers: vec![],
            peer_addrs: HashMap::new(),
            cluster_port: 0,
            advertise_addr: None,
            heartbeat_interval: Duration::from_millis(50),
            election_timeout_min: Duration::from_millis(100),
            election_timeout_max: Duration::from_millis(200),
            learner: false,
        },
        sled::Config::new().temporary(true).open().unwrap(),
    )
}
async fn leader() -> Arc<ClusterNode> {
    let node = node("leader");
    {
        let mut state = node.state.write().await;
        state.current_term = 1;
        state.role = Role::Leader;
        state.leader_id = Some("leader".into());
    }
    node
}
struct Committer(tokio::task::JoinHandle<()>);
impl Committer {
    fn new(node: Arc<ClusterNode>) -> Self {
        Self(tokio::spawn(async move {
            loop {
                node.sim_commit().await;
                tokio::task::yield_now().await;
            }
        }))
    }
}
impl Drop for Committer {
    fn drop(&mut self) {
        self.0.abort();
    }
}
fn request(id: &str) -> Publication {
    Publication {
        operation_id: id.into(),
        key: b"head\0binary".to_vec(),
        expected_revision: None,
        value: Some(b"value".to_vec()),
        companions: vec![Write::new("history", id.as_bytes(), b"event")],
    }
}

#[tokio::test]
async fn raft_metadata_binary_cas_and_atomic_companions() {
    let node = leader().await;
    let _committer = Committer::new(node.clone());
    let store = RaftMetadataStore::new(node.clone(), "namespace\0one").unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        assert!(
            store
                .compare_and_swap(
                    Write::new("heads\0", vec![0, 255], vec![0, 1, 255]),
                    None,
                    vec![Write::new("history", b"entry", b"event")]
                )
                .await
                .unwrap()
        );
        assert_eq!(
            store.get("heads\0", &[0, 255]).await.unwrap(),
            Some(vec![0, 1, 255])
        );
        assert!(
            !store
                .compare_and_swap(
                    Write::new("heads\0", vec![0, 255], b"wrong"),
                    None,
                    vec![Write::new("history", b"wrong", b"must not exist")]
                )
                .await
                .unwrap()
        );
        assert!(store.get("history", b"wrong").await.unwrap().is_none());
        assert_eq!(
            store.scan("heads\0", &[0], false, Some(1)).await.unwrap(),
            vec![(vec![0, 255], vec![0, 1, 255])]
        );
        assert_eq!(
            store.tree_names().await.unwrap(),
            vec!["heads\0".to_owned(), "history".to_owned()]
        );
        assert!(
            RaftMetadataStore::new(node, "other")
                .unwrap()
                .get("heads\0", &[0, 255])
                .await
                .unwrap()
                .is_none()
        );
    })
    .await
    .unwrap();
}
#[tokio::test]
async fn raft_metadata_ids_are_replicated_and_unique_under_contention() {
    let node = leader().await;
    let _committer = Committer::new(node.clone());
    let store = Arc::new(RaftMetadataStore::new(node.clone(), "ids").unwrap());
    let mut tasks = vec![];
    for _ in 0..8 {
        let store = store.clone();
        tasks.push(tokio::spawn(
            async move { store.generate_id().await.unwrap() },
        ))
    }
    let mut ids = vec![];
    tokio::time::timeout(Duration::from_secs(3), async {
        for task in tasks {
            ids.push(task.await.unwrap())
        }
    })
    .await
    .unwrap();
    ids.sort();
    assert_eq!(ids, (1..=8).collect::<Vec<_>>());
    let reopened = ClusterNode::new(node.config.clone(), node.db.clone());
    {
        let mut state = reopened.state.write().await;
        state.current_term = 2;
        state.role = Role::Leader;
        state.leader_id = Some("leader".into());
    }
    let _reopened_committer = Committer::new(reopened.clone());
    let store = RaftMetadataStore::new(reopened, "ids").unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(3), store.generate_id())
            .await
            .unwrap()
            .unwrap(),
        9
    );
    assert_eq!(
        store.tree_names().await.unwrap(),
        Vec::<String>::new(),
        "counter is not a user tree"
    );
}
#[tokio::test]
async fn pending_publication_conflict_is_provisional_and_replaced_proposal_is_unknown() {
    let node = leader().await;
    let metadata = MetadataPublication::new(
        Arc::new(RaftMetadataStore::new(node.clone(), "test").unwrap()),
        "test",
    )
    .unwrap();
    assert!(
        metadata
            .read(b"key", ReadMode::Linearizable)
            .await
            .unwrap_err()
            .contains("unsupported")
    );
    let first = metadata.clone();
    let proposal = tokio::spawn(async move { first.publish(&request("first")).await.unwrap() });
    tokio::time::timeout(Duration::from_secs(2), async {
        while node.state.read().await.log.is_empty() {
            tokio::task::yield_now().await
        }
    })
    .await
    .unwrap();
    assert_eq!(
        metadata.publish(&request("second")).await.unwrap(),
        PublishOutcome::ProvisionalConflict
    );
    assert!(
        node.handle_append_entries(AppendEntriesRequest {
            term: 2,
            leader_id: "replacement".into(),
            prev_log_index: 0,
            prev_log_term: 0,
            entries: vec![LogEntry {
                term: 2,
                index: 1,
                key: "other".into(),
                value: "replacement".into(),
                extra: vec![]
            }],
            leader_commit: 1,
            peer_addrs: None,
            learner_addrs: None
        })
        .await
        .success
    );
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(2), proposal)
            .await
            .unwrap()
            .unwrap(),
        PublishOutcome::UnknownOutcome { .. }
    ));
    assert!(
        metadata
            .read(&request("first").key, ReadMode::BackendDefault)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        metadata.replay(&request("first")).await.unwrap().is_none(),
        "missing receipt on local reads is not a failure proof"
    );
}
#[derive(Default)]
struct Objects(std::sync::Mutex<HashMap<String, Vec<u8>>>);
#[async_trait::async_trait]
impl mcp_metadata_store::blob::BlobStorage for Objects {
    async fn put(&self, k: &str, v: &[u8]) -> Result<(), String> {
        self.0.lock().unwrap().insert(k.into(), v.to_vec());
        Ok(())
    }
    async fn get(&self, k: &str) -> Result<Vec<u8>, String> {
        self.0
            .lock()
            .unwrap()
            .get(k)
            .cloned()
            .ok_or("missing object".into())
    }
    async fn put_immutable(&self, k: &str, v: &[u8]) -> Result<(), String> {
        let mut objects = self.0.lock().unwrap();
        match objects.get(k) {
            Some(old) if old != v => Err("immutable conflict".into()),
            Some(_) => Ok(()),
            None => {
                objects.insert(k.into(), v.to_vec());
                Ok(())
            }
        }
    }
}
#[tokio::test]
async fn follower_references_need_shared_blobs_and_do_not_replicate_payload_bytes() {
    use mcp_metadata_store::{
        content::{ContentObjects, ObjectKind, ObjectRef},
        publication::ObjectPublication,
    };
    let node = leader().await;
    let _committer = Committer::new(node.clone());
    let shared = Arc::new(Objects::default());
    let objects = ContentObjects::new(shared.clone());
    let metadata = MetadataPublication::new(
        Arc::new(RaftMetadataStore::new(node.clone(), "objects").unwrap()),
        "objects",
    )
    .unwrap();
    let publication = ObjectPublication {
        objects: objects.clone(),
        metadata,
    };
    let bytes = vec![9; 200_000];
    assert_eq!(
        tokio::time::timeout(
            Duration::from_secs(3),
            publication.publish(request("object"), ObjectKind::Artifact, &bytes)
        )
        .await
        .unwrap()
        .unwrap(),
        PublishOutcome::Committed { revision: 1 }
    );
    let entries = node.state.read().await.log.clone();
    assert!(
        entries
            .iter()
            .all(|e| e.value.len() < 1024 && e.extra.iter().all(|(_, v)| v.len() < 1024)),
        "Raft log must hold metadata, not payload bytes"
    );
    let follower = node_for_follower();
    assert!(
        follower
            .handle_append_entries(AppendEntriesRequest {
                term: 1,
                leader_id: "leader".into(),
                prev_log_index: 0,
                prev_log_term: 0,
                entries: entries.clone(),
                leader_commit: entries.last().unwrap().index,
                peer_addrs: None,
                learner_addrs: None
            })
            .await
            .success
    );
    let metadata = MetadataPublication::new(
        Arc::new(RaftMetadataStore::new(follower, "objects").unwrap()),
        "objects",
    )
    .unwrap();
    let record = metadata
        .read(&request("object").key, ReadMode::BackendDefault)
        .await
        .unwrap()
        .unwrap();
    let reference = ObjectRef::decode(&record.value.unwrap()).unwrap();
    assert_eq!(
        ContentObjects::new(shared).get(&reference).await.unwrap(),
        bytes
    );
    assert!(
        ContentObjects::new(Arc::new(Objects::default()))
            .get(&reference)
            .await
            .is_err(),
        "replicated references cannot manufacture node-local blobs"
    );
}
fn node_for_follower() -> Arc<ClusterNode> {
    node("follower")
}

#[tokio::test]
async fn raft_metadata_scan_applies_direction_and_limits_before_decoding_values() {
    let node = leader().await;
    let _committer = Committer::new(node.clone());
    let store = RaftMetadataStore::new(node.clone(), "scan").unwrap();
    store
        .put(Write::new("ordered", b"a", b"first"))
        .await
        .unwrap();
    store
        .put(Write::new("ordered", b"z", b"last"))
        .await
        .unwrap();
    let key = node.state.read().await.log[0].key.clone();
    let prefix = key.rsplit_once(":k:").unwrap().0;
    // A malformed middle value must not affect an intact bounded window.
    node.db
        .open_tree("data")
        .unwrap()
        .insert(format!("{prefix}:k:6d"), b"not-hex")
        .unwrap();
    assert_eq!(
        store.scan("ordered", b"", false, Some(1)).await.unwrap(),
        vec![(b"a".to_vec(), b"first".to_vec())]
    );
    assert_eq!(
        store.scan("ordered", b"", true, Some(1)).await.unwrap(),
        vec![(b"z".to_vec(), b"last".to_vec())]
    );
    assert!(
        store
            .scan("ordered", b"", false, Some(0))
            .await
            .unwrap()
            .is_empty()
    );
    assert!(store.scan("ordered", b"", false, None).await.is_err());
}
