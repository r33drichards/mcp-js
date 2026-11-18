//! Synchronous runtime metadata adapter. A dedicated I/O thread owns the async
//! runtime, so callers inside V8 or Tokio never nest `block_on`.
use crate::{
    MetadataStore, Result, Row, Write,
    content::{ContentObjects, ObjectKind, ObjectRef},
};
use std::sync::{Arc, mpsc};

type Job = Box<dyn FnOnce(&tokio::runtime::Runtime, &Arc<dyn MetadataStore>) + Send>;
#[derive(Clone)]
pub struct RuntimeStore {
    tx: mpsc::Sender<Job>,
    blobs: Option<Arc<dyn RuntimeBlobs>>,
}
/// Compatibility alias for the shared named-blob interface.
pub use crate::blob::BlobStorage as RuntimeBlobs;
impl RuntimeStore {
    pub fn new(store: Arc<dyn MetadataStore>) -> Result<Self> {
        Self::from_factory(async move { Ok((store, None)) })
    }
    /// Network clients must be constructed on this thread's runtime, not a
    /// caller runtime that may be blocked by a synchronous metadata request.
    pub fn from_factory<F>(factory: F) -> Result<Self>
    where
        F: std::future::Future<
                Output = Result<(Arc<dyn MetadataStore>, Option<Arc<dyn RuntimeBlobs>>)>,
            > + Send
            + 'static,
    {
        let (tx, rx) = mpsc::channel::<Job>();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("runtime-metadata".into())
            .spawn(move || {
                let initialized = (|| {
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(|e| e.to_string())?;
                    let (store, blobs) = runtime.block_on(factory)?;
                    Ok::<_, String>((runtime, store, blobs))
                })();
                match initialized {
                    Ok((runtime, store, blobs)) => {
                        if ready_tx.send(Ok(blobs)).is_err() {
                            return;
                        }
                        for job in rx {
                            job(&runtime, &store);
                        }
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        let blobs = ready_rx
            .recv()
            .map_err(|_| "runtime metadata initialization failed".to_string())??;
        Ok(Self { tx, blobs })
    }
    pub fn with_blobs(mut self, blobs: Arc<dyn RuntimeBlobs>) -> Self {
        self.blobs = Some(blobs);
        self
    }
    fn call<T: Send + 'static>(
        &self,
        f: impl FnOnce(&tokio::runtime::Runtime, &Arc<dyn MetadataStore>) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.tx
            .send(Box::new(move |rt, db| {
                let _ = tx.send(f(rt, db));
            }))
            .map_err(|_| "runtime metadata worker stopped")?;
        rx.recv()
            .map_err(|_| "runtime metadata worker failed".to_string())?
    }
    pub fn tree(&self, name: &str) -> RuntimeTree {
        let tree = RuntimeTree::Remote {
            store: self.clone(),
            name: name.into(),
        };
        if name == "artifacts" || name.starts_with("ex:") {
            tree.with_payload_store(self.clone())
        } else {
            tree
        }
    }
    fn put_object(&self, kind: ObjectKind, value: Vec<u8>) -> Result<ObjectRef> {
        let blobs = self.blobs.clone();
        self.call(move |rt, _| {
            wait(rt, async {
                ContentObjects::new(blobs.ok_or("blob storage is required for runtime payloads")?)
                    .put(kind, &value)
                    .await
            })
        })
    }
    fn get_object(&self, reference: ObjectRef) -> Result<Vec<u8>> {
        let blobs = self.blobs.clone();
        self.call(move |rt, _| {
            wait(rt, async {
                ContentObjects::new(
                    blobs.ok_or("blob storage is required to read runtime payloads")?,
                )
                .get(&reference)
                .await
            })
        })
    }
    fn put_payload(&self, kind: ObjectKind, value: Vec<u8>) -> Result<Vec<u8>> {
        let reference = self.put_object(kind, value)?;
        let mut bytes = CONTENT_REFERENCE.to_vec();
        bytes.extend(reference.encode());
        Ok(bytes)
    }
    fn read_payload(&self, value: Vec<u8>) -> Result<Vec<u8>> {
        if let Some(bytes) = value.strip_prefix(CONTENT_REFERENCE) {
            return self.get_object(ObjectRef::decode(bytes)?);
        }
        let Some(name) = value.strip_prefix(PAYLOAD_REFERENCE) else {
            // Existing inline records remain readable; new writes never use this form.
            return Ok(value);
        };
        let name = std::str::from_utf8(name)
            .map_err(|e| e.to_string())?
            .to_owned();
        if !name.starts_with("runtime/") {
            return Err("invalid runtime payload reference".into());
        }
        let blobs = self.blobs.clone();
        self.call(move |rt, _| {
            wait(rt, async {
                blobs
                    .ok_or("blob storage is required to read this runtime payload")?
                    .get(&name)
                    .await
            })
        })
    }
}

#[derive(Clone)]
pub enum RuntimeTree {
    Local(sled::Tree),
    Remote {
        store: RuntimeStore,
        name: String,
    },
    Payload {
        metadata: Box<RuntimeTree>,
        store: RuntimeStore,
        kind: ObjectKind,
    },
}
impl From<sled::Tree> for RuntimeTree {
    fn from(t: sled::Tree) -> Self {
        Self::Local(t)
    }
}
// Remote tombstones distinguish deletion from absent, preserving atomic CAS.
fn encode(value: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(value.len() + 1);
    v.push(1);
    v.extend_from_slice(value);
    v
}
fn decode(value: Option<Vec<u8>>) -> Result<Option<Vec<u8>>> {
    match value {
        None => Ok(None),
        Some(v) if v == [0] => Ok(None),
        Some(v) if v.first() == Some(&1) => Ok(Some(v[1..].to_vec())),
        _ => Err("invalid runtime metadata envelope".into()),
    }
}
const CONTENT_REFERENCE: &[u8] = b"\0mcp-runtime-content-v1\0";
const PAYLOAD_REFERENCE: &[u8] = b"\0mcp-runtime-blob-v1\0";
async fn decode_value(
    value: Option<Vec<u8>>,
    blobs: &Option<Arc<dyn RuntimeBlobs>>,
) -> Result<Option<Vec<u8>>> {
    if let Some(v) = &value {
        if v.first() == Some(&2) {
            let name = std::str::from_utf8(&v[1..]).map_err(|e| e.to_string())?;
            if !name.starts_with("runtime/") {
                return Err("invalid runtime blob reference".into());
            }
            return Ok(Some(
                blobs
                    .as_ref()
                    .ok_or("runtime blob storage missing")?
                    .get(name)
                    .await?,
            ));
        }
    }
    decode(value)
}
fn wait<T>(
    rt: &tokio::runtime::Runtime,
    future: impl std::future::Future<Output = Result<T>>,
) -> Result<T> {
    rt.block_on(async {
        tokio::time::timeout(std::time::Duration::from_secs(10), future)
            .await
            .map_err(|_| {
                "runtime metadata operation timed out; outcome may be uncertain".to_string()
            })?
    })
}
impl RuntimeTree {
    // SQLite/DynamoDB acknowledge durable writes themselves. Sled references
    // must be flushed before acknowledging a newly uploaded payload.
    fn flush_local(&self) -> Result<()> {
        match self {
            Self::Local(tree) => tree.flush().map(|_| ()).map_err(|e| e.to_string()),
            Self::Payload { metadata, .. } => metadata.flush_local(),
            Self::Remote { .. } => Ok(()),
        }
    }
    /// Store every payload in blob storage, publishing its database reference only after upload succeeds.
    pub fn with_payload_store(self, store: RuntimeStore) -> Self {
        let artifact = match &self {
            Self::Remote { name, .. } => name == "artifacts",
            Self::Local(tree) => tree.name().as_ref() == b"artifacts",
            Self::Payload { kind, .. } => *kind == ObjectKind::Artifact,
        };
        Self::Payload {
            metadata: Box::new(self),
            store,
            kind: if artifact {
                ObjectKind::Artifact
            } else {
                ObjectKind::ConsolePage
            },
        }
    }
    pub fn has_objects(&self) -> bool {
        matches!(self, Self::Payload { .. })
    }
    pub fn put_object(&self, kind: ObjectKind, value: &[u8]) -> Result<ObjectRef> {
        match self {
            Self::Payload { store, .. } => store.put_object(kind, value.to_vec()),
            _ => Err("runtime object storage unavailable".into()),
        }
    }
    pub fn get_object(&self, reference: ObjectRef) -> Result<Vec<u8>> {
        match self {
            Self::Payload { store, .. } => store.get_object(reference),
            _ => Err("runtime object storage unavailable".into()),
        }
    }
    pub fn insert_metadata(&self, key: impl AsRef<[u8]>, value: impl AsRef<[u8]>) -> Result<()> {
        match self {
            Self::Payload { metadata, .. } => {
                metadata.insert(key, value)?;
                metadata.flush_local()
            }
            _ => self.insert(key, value),
        }
    }
    pub fn get(&self, key: impl AsRef<[u8]>) -> Result<Option<Vec<u8>>> {
        let key = key.as_ref().to_vec();
        match self {
            Self::Payload {
                metadata, store, ..
            } => metadata
                .get(key)?
                .map(|v| store.read_payload(v))
                .transpose(),
            Self::Local(t) => t
                .get(key)
                .map(|v| v.map(|v| v.to_vec()))
                .map_err(|e| e.to_string()),
            Self::Remote { store, name } => {
                let name = name.clone();
                let blobs = store.blobs.clone();
                store.call(move |rt, db| {
                    wait(rt, async {
                        decode_value(db.get(&name, &key).await?, &blobs).await
                    })
                })
            }
        }
    }
    pub fn insert(&self, key: impl AsRef<[u8]>, value: impl AsRef<[u8]>) -> Result<()> {
        let key = key.as_ref().to_vec();
        let value = value.as_ref().to_vec();
        match self {
            Self::Payload {
                metadata,
                store,
                kind,
            } => {
                metadata.insert(key, store.put_payload(*kind, value)?)?;
                metadata.flush_local()
            }
            Self::Local(t) => t.insert(key, value).map(|_| ()).map_err(|e| e.to_string()),
            Self::Remote { store, name } => {
                let name = name.clone();
                store.call(move |rt, db| {
                    wait(rt, async {
                        db.put(Write::new(&name, key, encode(&value))).await
                    })
                })
            }
        }
    }
    pub fn remove(&self, key: impl AsRef<[u8]>) -> Result<()> {
        let key = key.as_ref().to_vec();
        match self {
            Self::Payload { metadata, .. } => {
                metadata.remove(key)?;
                metadata.flush_local()
            }
            Self::Local(t) => t.remove(key).map(|_| ()).map_err(|e| e.to_string()),
            Self::Remote { store, name } => {
                let name = name.clone();
                store.call(move |rt, db| wait(rt, db.put(Write::new(&name, key, vec![0]))))
            }
        }
    }
    pub fn compare_and_swap(
        &self,
        key: impl AsRef<[u8]>,
        old: Option<Vec<u8>>,
        new: Option<Vec<u8>>,
    ) -> Result<std::result::Result<(), ()>> {
        let key = key.as_ref().to_vec();
        let ok = match self {
            Self::Payload {
                metadata,
                store,
                kind,
            } => {
                let current = metadata.get(&key)?;
                if current.clone().map(|v| store.read_payload(v)).transpose()? != old {
                    return Ok(Err(()));
                }
                let replacement = new.map(|v| store.put_payload(*kind, v)).transpose()?;
                let ok = metadata
                    .compare_and_swap(key, current, replacement)?
                    .is_ok();
                if ok {
                    metadata.flush_local()?;
                }
                ok
            }
            Self::Local(t) => t
                .compare_and_swap(key, old, new)
                .map(|v| v.is_ok())
                .map_err(|e| e.to_string())?,
            Self::Remote { store, name } => {
                let name = name.clone();
                let blobs = store.blobs.clone();
                store.call(move |rt, db| {
                    wait(rt, async {
                        let current = db.get(&name, &key).await?;
                        if decode_value(current.clone(), &blobs).await? != old {
                            return Ok(false);
                        }
                        let encoded = match new {
                            Some(v) => encode(&v),
                            None => vec![0],
                        };
                        db.compare_and_swap(Write::new(&name, key, encoded), current, vec![])
                            .await
                    })
                })?
            }
        };
        Ok(if ok { Ok(()) } else { Err(()) })
    }
    pub fn iter(&self) -> std::vec::IntoIter<Result<Row>> {
        let rows: Result<Vec<Row>> = match self {
            Self::Payload {
                metadata, store, ..
            } => metadata
                .iter()
                .map(|row| {
                    let (key, value) = row?;
                    Ok((key, store.read_payload(value)?))
                })
                .collect(),
            Self::Local(t) => t
                .iter()
                .map(|r| {
                    r.map(|(k, v)| (k.to_vec(), v.to_vec()))
                        .map_err(|e| e.to_string())
                })
                .collect(),
            Self::Remote { store, name } => {
                let name = name.clone();
                let blobs = store.blobs.clone();
                store.call(move |rt, db| {
                    wait(rt, async {
                        let rows = db.scan(&name, b"", false, None).await?;
                        let mut out = Vec::new();
                        for (k, v) in rows {
                            if let Some(v) = decode_value(Some(v), &blobs).await? {
                                out.push((k, v));
                            }
                        }
                        Ok(out)
                    })
                })
            }
        };
        match rows {
            Ok(rows) => rows.into_iter().map(Ok).collect::<Vec<_>>(),
            Err(e) => vec![Err(e)],
        }
        .into_iter()
    }
}
