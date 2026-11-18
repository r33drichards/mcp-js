use mcp_metadata_store::{
    blob::BlobStorage,
    content::{ContentObjects, ObjectKind, ObjectRef},
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
#[derive(Default)]
struct Backend(Mutex<HashMap<String, Vec<u8>>>);
#[async_trait::async_trait]
impl BlobStorage for Backend {
    async fn put(&self, key: &str, value: &[u8]) -> Result<(), String> {
        self.0.lock().unwrap().insert(key.into(), value.to_vec());
        Ok(())
    }
    async fn get(&self, key: &str) -> Result<Vec<u8>, String> {
        self.0
            .lock()
            .unwrap()
            .get(key)
            .cloned()
            .ok_or("missing".into())
    }
    async fn put_immutable(&self, key: &str, value: &[u8]) -> Result<(), String> {
        let mut map = self.0.lock().unwrap();
        match map.get(key) {
            Some(v) if v != value => Err("conflict".into()),
            Some(_) => Ok(()),
            None => {
                map.insert(key.into(), value.to_vec());
                Ok(())
            }
        }
    }
}
#[tokio::test]
async fn content_identity_roundtrips_and_detects_corruption() {
    let backend = Arc::new(Backend::default());
    let objects = ContentObjects::new(backend.clone());
    let a = objects
        .put(ObjectKind::Artifact, b"same bytes")
        .await
        .unwrap();
    assert_eq!(
        a,
        objects
            .put(ObjectKind::Artifact, b"same bytes")
            .await
            .unwrap()
    );
    assert_eq!(backend.0.lock().unwrap().len(), 1);
    let page = objects
        .put(ObjectKind::ConsolePage, b"same bytes")
        .await
        .unwrap();
    assert_ne!(a.key(), page.key());
    assert_eq!(ObjectRef::decode(&a.encode()).unwrap(), a);
    assert_eq!(objects.get(&a).await.unwrap(), b"same bytes");
    backend.put(&a.key(), b"corruption").await.unwrap();
    assert!(objects.get(&a).await.unwrap_err().contains("integrity"));
    assert!(
        objects
            .put(ObjectKind::Artifact, b"same bytes")
            .await
            .is_err()
    );
    assert!(ObjectRef::decode(&[0; 42]).is_err());
    assert!(ObjectRef::decode(&[1; 41]).is_err());
}
