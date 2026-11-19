//! Runtime metadata with explicit versioned payload indexes. Legacy inline bytes
//! are never inspected for a magic prefix. Blob work has its own bounded queue.
pub use crate::blob::BlobStorage as RuntimeBlobs;
use crate::{
    MetadataStore, Result, Row, Write,
    content::{ContentObjects, ObjectKind, ObjectRef},
    runtime_worker::{BlobWorker, MetadataWorker},
};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
#[derive(Clone)]
pub struct RuntimeStore {
    metadata: MetadataWorker,
    blobs: Option<Arc<BlobWorker>>,
}
impl RuntimeStore {
    pub fn new(store: Arc<dyn MetadataStore>) -> Result<Self> {
        Self::from_factory(async move { Ok((store, None)) })
    }
    /// Construct network clients on the metadata reactor, which is continuously
    /// driven even while the separate blob pool is awaiting those clients' I/O.
    pub fn from_factory<F>(factory: F) -> Result<Self>
    where
        F: std::future::Future<
                Output = Result<(Arc<dyn MetadataStore>, Option<Arc<dyn RuntimeBlobs>>)>,
            > + Send
            + 'static,
    {
        let (metadata, backend) = MetadataWorker::new(factory)?;
        let blobs = backend.map(|b| Arc::new(BlobWorker::new(b, metadata.clone())));
        Ok(Self { metadata, blobs })
    }
    pub fn with_blobs(mut self, backend: Arc<dyn RuntimeBlobs>) -> Self {
        self.blobs = Some(Arc::new(BlobWorker::new(backend, self.metadata.clone())));
        self
    }
    pub fn with_blob_limits(
        mut self,
        concurrency: usize,
        queue: usize,
        timeout: Duration,
    ) -> Result<Self> {
        let current = self.blobs.as_ref().ok_or("blob backend missing")?;
        self.blobs = Some(Arc::new(current.limits(concurrency, queue, timeout)?));
        Ok(self)
    }
    pub fn tree(&self, name: &str) -> RuntimeTree {
        let tree = RuntimeTree::Remote {
            store: self.clone(),
            name: name.into(),
        };
        // from_store remains a working inline constructor until a blob backend
        // is explicitly attached; server bootstrap always supplies one.
        if self.blobs.is_some() && (name == "artifacts" || name.starts_with("ex:")) {
            tree.with_payload_store(self.clone())
        } else {
            tree
        }
    }
    fn read_lease(&self) -> Result<Arc<dyn crate::blob::BlobLease>> {
        match &self.blobs {
            Some(worker) => worker.bound()?.0.runtime_read_lease(),
            None => Ok(Arc::new(())),
        }
    }
    /// Explicit local maintenance. Every metadata scope registered at this root
    /// must participate. Active readers/writers make collection fail as busy.
    pub fn collect_runtime_objects(
        stores: &[RuntimeStore],
        minimum_age: Duration,
    ) -> Result<usize> {
        let first = stores.first().ok_or("no runtime stores provided")?;
        let worker = first.blobs.as_ref().ok_or("blob backend unavailable")?;
        let (backend, _) = worker.bound()?;
        let identity = backend
            .runtime_gc_identity()
            .ok_or("runtime collection is unsupported")?;
        let mut owners = std::collections::BTreeSet::new();
        for store in stores {
            let (candidate, owner) = store
                .blobs
                .as_ref()
                .ok_or("blob backend unavailable")?
                .bound()?;
            if candidate.runtime_gc_identity().as_ref() != Some(&identity) {
                return Err("runtime collection requires one shared blob root".into());
            }
            owners.insert(owner.ok_or("runtime owner unavailable")?);
        }
        let mut roots = || {
            let mut reachable = std::collections::BTreeSet::new();
            for store in stores {
                let names = store
                    .metadata
                    .call(|db| async move { db.tree_names().await })?;
                for name in names {
                    if let Some(hex_name) = name.strip_prefix("runtime_refs:v1:") {
                        let logical =
                            String::from_utf8(hex::decode(hex_name).map_err(|e| e.to_string())?)
                                .map_err(|e| e.to_string())?;
                        for (_, value) in store.tree(&logical).entries()? {
                            match value {
                                Value::Object(reference,_,_,_)|Value::Metadata(reference,_)=>{reachable.insert(reference.key());},
                                Value::Inline(_) | Value::LegacyBlob(_,_) if logical=="artifacts"=>return Err("legacy artifact metadata must be migrated before runtime collection".into()),
                                _=>(),
                            }
                        }
                    } else if name == "artifacts" {
                        // An unindexed legacy artifact may contain an opaque
                        // reference. Collection must not guess its format.
                        for (_, value) in store.tree("artifacts").entries()? {
                            if matches!(value, Value::Inline(_) | Value::LegacyBlob(_, _)) {
                                return Err("legacy artifact metadata must be migrated before runtime collection".into());
                            }
                        }
                    }
                }
            }
            Ok(reachable)
        };
        backend.collect_runtime_objects(&owners, minimum_age, &mut roots)
    }
    fn put_object(&self, kind: ObjectKind, bytes: Vec<u8>) -> Result<ObjectRef> {
        self.blobs
            .as_ref()
            .ok_or("runtime blob storage unavailable")?
            .call(
                move |backend| async move { ContentObjects::new(backend).put(kind, &bytes).await },
            )
    }
    fn get_object(&self, reference: ObjectRef) -> Result<Vec<u8>> {
        self.blobs
            .as_ref()
            .ok_or("runtime blob storage unavailable")?
            .call(move |backend| async move { ContentObjects::new(backend).get(&reference).await })
    }
    fn legacy_blob(&self, name: String) -> Result<Vec<u8>> {
        validate_legacy_reference(&name)?;
        self.blobs
            .as_ref()
            .ok_or("legacy blob storage unavailable")?
            .call(move |backend| async move { backend.get(&name).await })
    }
}
fn validate_legacy_reference(name: &str) -> Result<()> {
    let suffix = name
        .strip_prefix("runtime/")
        .ok_or("invalid legacy blob reference")?;
    let uuid = uuid::Uuid::parse_str(suffix).map_err(|_| "invalid legacy blob reference")?;
    if uuid.hyphenated().to_string() != suffix {
        return Err("invalid legacy blob reference".into());
    }
    Ok(())
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
        index: Box<RuntimeTree>,
        store: RuntimeStore,
        kind: ObjectKind,
    },
}
impl From<sled::Tree> for RuntimeTree {
    fn from(tree: sled::Tree) -> Self {
        Self::Local(tree)
    }
}
#[derive(Clone)]
enum Value {
    Inline(Vec<u8>),
    LegacyBlob(String, RuntimeStore),
    Object(ObjectRef, u64, bool, RuntimeStore),
    Metadata(ObjectRef, Vec<u8>),
    Deleted,
}
impl Value {
    fn read(&self) -> Result<Option<Vec<u8>>> {
        match self {
            Self::Inline(v) => Ok(Some(v.clone())),
            Self::LegacyBlob(name, store) => store.legacy_blob(name.clone()).map(Some),
            Self::Object(reference, _, _, store) => store.get_object(reference.clone()).map(Some),
            Self::Metadata(_, bytes) => Ok(Some(bytes.clone())),
            Self::Deleted => Ok(None),
        }
    }
}
fn decode_remote(bytes: Vec<u8>, store: &RuntimeStore) -> Result<Value> {
    match bytes.first() {
        Some(0) if bytes.len() == 1 => Ok(Value::Deleted),
        Some(1) => Ok(Value::Inline(bytes[1..].to_vec())),
        Some(2) => {
            let name = String::from_utf8(bytes[1..].to_vec()).map_err(|e| e.to_string())?;
            validate_legacy_reference(&name)?;
            Ok(Value::LegacyBlob(name, store.clone()))
        }
        _ => Err("invalid runtime metadata envelope".into()),
    }
}
fn decode_index(bytes: Vec<u8>, store: &RuntimeStore) -> Result<Value> {
    match bytes.first() {
        Some(0) if bytes.len() == 1 => Ok(Value::Deleted),
        Some(1) => Ok(Value::Inline(bytes[1..].to_vec())),
        Some(3) if bytes.len() == 52 => {
            let reference = ObjectRef::decode(&bytes[1..43])?;
            let lines = u64::from_be_bytes(bytes[43..51].try_into().unwrap());
            if bytes[51] > 1 || lines > reference.size_bytes {
                return Err("invalid console page statistics".into());
            }
            Ok(Value::Object(
                reference,
                lines,
                bytes[51] != 0,
                store.clone(),
            ))
        }
        Some(4) if bytes.len() >= 43 => Ok(Value::Metadata(
            ObjectRef::decode(&bytes[1..43])?,
            bytes[43..].to_vec(),
        )),
        _ => Err("invalid versioned runtime reference".into()),
    }
}
fn inline(bytes: &[u8]) -> Vec<u8> {
    let mut v = vec![1];
    v.extend(bytes);
    v
}
/// Small metadata-only descriptor. Reading one descriptor fetches at most its
/// one object; scanning descriptors never fetches the rest of a console stream.
pub struct RuntimePage {
    pub key: Vec<u8>,
    pub length: u64,
    pub newlines: u64,
    pub ends_with_newline: bool,
    value: Value,
    _lease: Arc<dyn crate::blob::BlobLease>,
}
impl RuntimePage {
    pub fn read(&self) -> Result<Vec<u8>> {
        self.value.read()?.ok_or("deleted runtime page".into())
    }
}
impl RuntimeTree {
    fn name(&self) -> Vec<u8> {
        match self {
            Self::Local(t) => t.name().to_vec(),
            Self::Remote { name, .. } => name.as_bytes().to_vec(),
            Self::Payload { metadata, .. } => metadata.name(),
        }
    }
    pub fn with_payload_store(self, store: RuntimeStore) -> Self {
        let name = self.name();
        let kind = if name == b"artifacts" {
            ObjectKind::Artifact
        } else {
            ObjectKind::ConsolePage
        };
        let index = Self::Remote {
            store: store.clone(),
            name: format!("runtime_refs:v1:{}", hex::encode(name)),
        };
        Self::Payload {
            metadata: Box::new(self),
            index: Box::new(index),
            store,
            kind,
        }
    }
    pub fn has_objects(&self) -> bool {
        matches!(self, Self::Payload { .. })
    }
    pub fn put_object(&self, kind: ObjectKind, bytes: &[u8]) -> Result<ObjectRef> {
        match self {
            Self::Payload { store, .. } => store.put_object(kind, bytes.to_vec()),
            _ => Err("runtime object storage unavailable".into()),
        }
    }
    pub fn get_object(&self, reference: ObjectRef) -> Result<Vec<u8>> {
        match self {
            Self::Payload { store, .. } => store.get_object(reference),
            _ => Err("runtime object storage unavailable".into()),
        }
    }
    fn flush(&self) -> Result<()> {
        match self {
            Self::Local(t) => t.flush().map(|_| ()).map_err(|e| e.to_string()),
            Self::Remote { store, .. } => store.metadata.call(|db| async move { db.flush() }),
            Self::Payload { index, .. } => index.flush(),
        }
    }
    fn value(&self, key: &[u8]) -> Result<Option<Value>> {
        match self {
            Self::Local(t) => t
                .get(key)
                .map(|v| v.map(|v| Value::Inline(v.to_vec())))
                .map_err(|e| e.to_string()),
            Self::Remote { store, name } => {
                let name = name.clone();
                let key = key.to_vec();
                store
                    .metadata
                    .call(move |db| async move { db.get(&name, &key).await })?
                    .map(|v| decode_remote(v, store))
                    .transpose()
            }
            Self::Payload {
                metadata,
                index,
                store,
                ..
            } => match index.get(key)? {
                Some(v) => Ok(Some(decode_index(v, store)?)),
                None => metadata.value(key),
            },
        }
    }
    pub fn read_lease(&self) -> Result<Arc<dyn crate::blob::BlobLease>> {
        match self {
            Self::Payload { store, .. } => store.read_lease(),
            _ => Ok(Arc::new(())),
        }
    }
    pub fn get_with_object(
        &self,
        key: impl AsRef<[u8]>,
    ) -> Result<Option<(Vec<u8>, Option<ObjectRef>)>> {
        let _lease = self.read_lease()?;
        match self.value(key.as_ref())? {
            Some(Value::Metadata(reference, bytes)) => Ok(Some((bytes, Some(reference)))),
            Some(other) => other.read().map(|v| v.map(|v| (v, None))),
            None => Ok(None),
        }
    }
    pub fn iter_with_objects(
        &self,
    ) -> Box<dyn Iterator<Item = Result<(Vec<u8>, Vec<u8>, Option<ObjectRef>)>> + Send> {
        let lease = match self.read_lease() {
            Ok(v) => v,
            Err(e) => return Box::new(std::iter::once(Err(e))),
        };
        match self.entries() {
            Ok(rows) => Box::new(
                rows.into_iter()
                    .filter(|(_, v)| !matches!(v, Value::Deleted))
                    .map(move |(key, value)| {
                        let _lease = &lease;
                        match value {
                            Value::Metadata(reference, bytes) => Ok((key, bytes, Some(reference))),
                            other => Ok((key, other.read()?.ok_or("deleted runtime row")?, None)),
                        }
                    }),
            ),
            Err(e) => Box::new(std::iter::once(Err(e))),
        }
    }
    pub fn get(&self, key: impl AsRef<[u8]>) -> Result<Option<Vec<u8>>> {
        let _lease = self.read_lease()?;
        self.value(key.as_ref())?
            .map(|v| v.read())
            .transpose()
            .map(Option::flatten)
    }
    pub fn insert_metadata(&self, key: impl AsRef<[u8]>, value: impl AsRef<[u8]>) -> Result<()> {
        self.insert_object_metadata(key, value, None)
    }
    pub fn insert_object_metadata(
        &self,
        key: impl AsRef<[u8]>,
        value: impl AsRef<[u8]>,
        reference: Option<ObjectRef>,
    ) -> Result<()> {
        let _lease = self.read_lease()?;
        match self {
            Self::Payload { index, .. } => {
                let bytes = match reference {
                    Some(reference) => {
                        let mut bytes = vec![4];
                        bytes.extend(reference.encode());
                        bytes.extend(value.as_ref());
                        bytes
                    }
                    None => inline(value.as_ref()),
                };
                index.insert(key, bytes)?;
                index.flush()
            }
            _ if reference.is_some() => Err("object-backed metadata tree required".into()),
            _ => self.insert(key, value),
        }
    }
    fn encoded_object(&self, value: Vec<u8>) -> Result<Vec<u8>> {
        let Self::Payload { store, kind, .. } = self else {
            return Err("not an object-backed tree".into());
        };
        let lines = value.iter().filter(|&&b| b == b'\n').count() as u64;
        let ends = value.last() == Some(&b'\n');
        let reference = store.put_object(*kind, value)?;
        let mut record = vec![3];
        record.extend(reference.encode());
        record.extend(lines.to_be_bytes());
        record.push(u8::from(ends));
        Ok(record)
    }
    pub fn insert(&self, key: impl AsRef<[u8]>, value: impl AsRef<[u8]>) -> Result<()> {
        let _lease = self.read_lease()?;
        let key = key.as_ref().to_vec();
        let bytes = value.as_ref().to_vec();
        match self {
            Self::Local(t) => t.insert(key, bytes).map(|_| ()).map_err(|e| e.to_string()),
            Self::Remote { store, name } => {
                let name = name.clone();
                store.metadata.call(move |db| async move {
                    db.put(Write::new(&name, key, inline(&bytes))).await
                })
            }
            Self::Payload { index, .. } => {
                let record = self.encoded_object(bytes)?;
                index.insert(key, record)?;
                index.flush()
            }
        }
    }
    pub fn remove(&self, key: impl AsRef<[u8]>) -> Result<()> {
        let _lease = self.read_lease()?;
        let key = key.as_ref().to_vec();
        match self {
            Self::Local(t) => t.remove(key).map(|_| ()).map_err(|e| e.to_string()),
            Self::Remote { store, name } => {
                let name = name.clone();
                store
                    .metadata
                    .call(move |db| async move { db.put(Write::new(&name, key, vec![0])).await })
            }
            Self::Payload { index, .. } => {
                index.insert(key, vec![0])?;
                index.flush()
            }
        }
    }
    pub fn compare_and_swap(
        &self,
        key: impl AsRef<[u8]>,
        old: Option<Vec<u8>>,
        new: Option<Vec<u8>>,
    ) -> Result<std::result::Result<(), ()>> {
        let _lease = self.read_lease()?;
        let key = key.as_ref().to_vec();
        let applied = match self {
            Self::Local(t) => t
                .compare_and_swap(key, old, new)
                .map(|r| r.is_ok())
                .map_err(|e| e.to_string())?,
            Self::Remote { store, name } => {
                let name = name.clone();
                let store = store.clone();
                let raw = store.metadata.call({
                    let key = key.clone();
                    let name = name.clone();
                    move |db| async move { db.get(&name, &key).await }
                })?;
                let current = raw
                    .clone()
                    .map(|v| decode_remote(v, &store)?.read())
                    .transpose()?
                    .flatten();
                if current != old {
                    return Ok(Err(()));
                }
                store.metadata.call(move |db| async move {
                    db.compare_and_swap(
                        Write::new(&name, key, new.map(|v| inline(&v)).unwrap_or(vec![0])),
                        raw,
                        vec![],
                    )
                    .await
                })?
            }
            Self::Payload { index, .. } => {
                let current = index.get(&key)?;
                if self.get(&key)? != old {
                    return Ok(Err(()));
                }
                let replacement = match new {
                    Some(v) => self.encoded_object(v)?,
                    None => vec![0],
                };
                let success = index
                    .compare_and_swap(key, current, Some(replacement))?
                    .is_ok();
                if success {
                    index.flush()?
                }
                success
            }
        };
        Ok(if applied { Ok(()) } else { Err(()) })
    }
    fn entries(&self) -> Result<Vec<(Vec<u8>, Value)>> {
        match self {
            Self::Local(t) => t
                .iter()
                .map(|r| {
                    r.map(|(k, v)| (k.to_vec(), Value::Inline(v.to_vec())))
                        .map_err(|e| e.to_string())
                })
                .collect(),
            Self::Remote { store, name } => {
                let name = name.clone();
                let rows = store
                    .metadata
                    .call(move |db| async move { db.scan(&name, b"", false, None).await })?;
                rows.into_iter()
                    .map(|(k, v)| Ok((k, decode_remote(v, store)?)))
                    .collect()
            }
            Self::Payload {
                metadata,
                index,
                store,
                ..
            } => {
                let mut rows: BTreeMap<_, _> = metadata.entries()?.into_iter().collect();
                for (key, value) in index.entries()? {
                    let Value::Inline(bytes) = value else {
                        return Err("invalid reference index envelope".into());
                    };
                    rows.insert(key, decode_index(bytes, store)?);
                }
                Ok(rows.into_iter().collect())
            }
        }
    }
    pub fn pages(&self) -> Result<Vec<RuntimePage>> {
        let lease = self.read_lease()?;
        self.entries()?
            .into_iter()
            .filter(|(_, v)| !matches!(v, Value::Deleted))
            .map(|(key, value)| {
                let (length, newlines, ends, value) = match value {
                    Value::Object(ref r, lines, ends, _) => (r.size_bytes, lines, ends, value),
                    other => {
                        let bytes = other.read()?.ok_or("deleted runtime page")?;
                        (
                            bytes.len() as u64,
                            bytes.iter().filter(|&&b| b == b'\n').count() as u64,
                            bytes.last() == Some(&b'\n'),
                            Value::Inline(bytes),
                        )
                    }
                };
                Ok(RuntimePage {
                    key,
                    length,
                    newlines,
                    ends_with_newline: ends,
                    value,
                    _lease: lease.clone(),
                })
            })
            .collect()
    }
    pub fn iter(&self) -> Box<dyn Iterator<Item = Result<Row>> + Send> {
        let lease = match self.read_lease() {
            Ok(lease) => lease,
            Err(error) => return Box::new(std::iter::once(Err(error))),
        };
        match self.entries() {
            Ok(rows) => Box::new(
                rows.into_iter()
                    .filter(|(_, v)| !matches!(v, Value::Deleted))
                    .map(move |(key, value)| {
                        let _lease = &lease;
                        Ok((key, value.read()?.ok_or("deleted runtime row")?))
                    }),
            ),
            Err(error) => Box::new(std::iter::once(Err(error))),
        }
    }
}
