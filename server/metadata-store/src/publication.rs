//! Revisioned publication with durable, per-key operation receipts.
//! Object I/O precedes metadata commitment; uncertain acknowledgements are not conflicts.
use crate::{
    MetadataStore, Result, Write,
    content::{ContentObjects, ObjectKind, ObjectRef},
};
use sha2::{Digest, Sha256};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadMode {
    BackendDefault,
    Linearizable,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub revision: u64,
    pub value: Option<Vec<u8>>,
}
impl Record {
    fn encode(&self) -> Vec<u8> {
        let mut bytes = b"PUB1".to_vec();
        bytes.extend(self.revision.to_be_bytes());
        bytes.push(u8::from(self.value.is_some()));
        if let Some(value) = &self.value {
            bytes.extend(value)
        }
        bytes
    }
    fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < 13
            || &bytes[..4] != b"PUB1"
            || bytes[12] > 1
            || (bytes[12] == 0 && bytes.len() != 13)
        {
            return Err("invalid publication record".into());
        }
        let revision = u64::from_be_bytes(bytes[4..12].try_into().unwrap());
        if revision == 0 {
            return Err("invalid publication revision".into());
        }
        Ok(Self {
            revision,
            value: if bytes[12] == 1 {
                Some(bytes[13..].to_vec())
            } else {
                None
            },
        })
    }
}
#[derive(Clone, Debug)]
pub struct Publication {
    pub operation_id: String,
    pub key: Vec<u8>,
    pub expected_revision: Option<u64>,
    pub value: Option<Vec<u8>>,
    pub companions: Vec<Write>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PublishOutcome {
    Committed { revision: u64 },
    Conflict,
    ProvisionalConflict,
    UnknownOutcome { operation_id: String, cause: String },
}
#[derive(Clone)]
pub struct MetadataPublication {
    store: Arc<dyn MetadataStore>,
    prefix: String,
}
impl MetadataPublication {
    pub fn new(store: Arc<dyn MetadataStore>, namespace: &str) -> Result<Self> {
        if namespace.is_empty() || namespace.len() > 128 {
            return Err("invalid publication namespace".into());
        }
        Ok(Self {
            store,
            prefix: format!("publication/v1/{}", hex::encode(namespace)),
        })
    }
    fn records(&self) -> String {
        format!("{}/records", self.prefix)
    }
    fn operations(&self) -> String {
        format!("{}/operations", self.prefix)
    }
    pub fn index_tree(&self, tree: &str) -> String {
        format!("{}/index/{}", self.prefix, hex::encode(tree))
    }
    fn operation_key(request: &Publication) -> Vec<u8> {
        let mut key = (request.key.len() as u64).to_be_bytes().to_vec();
        key.extend(&request.key);
        key.extend(request.operation_id.as_bytes());
        key
    }
    fn fingerprint(request: &Publication) -> [u8; 32] {
        fn field(hash: &mut Sha256, bytes: &[u8]) {
            hash.update((bytes.len() as u64).to_be_bytes());
            hash.update(bytes)
        }
        let mut hash = Sha256::new();
        field(&mut hash, &request.key);
        hash.update([u8::from(request.expected_revision.is_some())]);
        hash.update(request.expected_revision.unwrap_or(0).to_be_bytes());
        hash.update([u8::from(request.value.is_some())]);
        field(&mut hash, request.value.as_deref().unwrap_or_default());
        hash.update((request.companions.len() as u64).to_be_bytes());
        for write in &request.companions {
            field(&mut hash, write.tree.as_bytes());
            field(&mut hash, &write.key);
            field(&mut hash, &write.value)
        }
        hash.finalize().into()
    }
    fn validate(request: &Publication) -> Result<()> {
        if request.operation_id.is_empty() || request.operation_id.len() > 128 {
            return Err("operation ID must contain 1..128 bytes".into());
        }
        let mut keys = std::collections::BTreeSet::new();
        if request
            .companions
            .iter()
            .any(|w| !keys.insert((&w.tree, &w.key)))
        {
            return Err("duplicate companion key".into());
        }
        Ok(())
    }
    fn unknown(request: &Publication, cause: String) -> PublishOutcome {
        PublishOutcome::UnknownOutcome {
            operation_id: request.operation_id.clone(),
            cause,
        }
    }
    fn conflict(&self) -> PublishOutcome {
        if self.store.provisional_conflicts() {
            PublishOutcome::ProvisionalConflict
        } else {
            PublishOutcome::Conflict
        }
    }
    pub async fn read(&self, key: &[u8], mode: ReadMode) -> Result<Option<Record>> {
        if mode == ReadMode::Linearizable && !self.store.supports_linearizable_reads() {
            return Err("linearizable reads are unsupported by this metadata backend".into());
        }
        self.store
            .get(&self.records(), key)
            .await?
            .map(|bytes| Record::decode(&bytes))
            .transpose()
    }
    /// A missing receipt is not a cancellation or failure proof: an earlier
    /// mutation may still be in flight, or this replica's read may be stale.
    pub async fn replay(&self, request: &Publication) -> Result<Option<PublishOutcome>> {
        Self::validate(request)?;
        let Some(bytes) = self
            .store
            .get(&self.operations(), &Self::operation_key(request))
            .await?
        else {
            return Ok(None);
        };
        if bytes.len() != 40 || bytes[..32] != Self::fingerprint(request) {
            return Err("operation ID reused with a different request or invalid receipt".into());
        }
        let revision = u64::from_be_bytes(bytes[32..40].try_into().unwrap());
        if revision == 0 {
            return Err("invalid publication receipt".into());
        }
        if let Err(error) = self.store.flush() {
            return Ok(Some(Self::unknown(request, error)));
        }
        Ok(Some(PublishOutcome::Committed { revision }))
    }
    pub async fn publish(&self, request: &Publication) -> Result<PublishOutcome> {
        Self::validate(request)?;
        if let Some(outcome) = self.replay(request).await? {
            return Ok(outcome);
        }
        let current = self.store.get(&self.records(), &request.key).await?;
        let revision = current
            .as_deref()
            .map(Record::decode)
            .transpose()?
            .map(|r| r.revision);
        // Read the operation receipt AFTER the primary snapshot. An intervening
        // publication changes the primary, making the final CAS fail; it cannot
        // overwrite a reused operation receipt with a different request.
        if let Some(outcome) = self.replay(request).await? {
            return Ok(outcome);
        }
        if revision != request.expected_revision {
            return Ok(self.conflict());
        }
        let next = revision
            .unwrap_or(0)
            .checked_add(1)
            .ok_or("publication revision exhausted")?;
        let mut receipt = Self::fingerprint(request).to_vec();
        receipt.extend(next.to_be_bytes());
        let mut companions = vec![Write::new(
            &self.operations(),
            Self::operation_key(request),
            receipt,
        )];
        companions.extend(
            request
                .companions
                .iter()
                .map(|w| Write::new(&self.index_tree(&w.tree), w.key.clone(), w.value.clone())),
        );
        let record = Record {
            revision: next,
            value: request.value.clone(),
        };
        let primary = Write::new(&self.records(), request.key.clone(), record.encode());
        self.store
            .validate_transaction(&primary, current.as_deref(), &companions)?;
        match self
            .store
            .compare_and_swap(primary, current, companions)
            .await
        {
            Ok(true) => match self.store.flush() {
                Ok(()) => Ok(PublishOutcome::Committed { revision: next }),
                Err(error) => Ok(Self::unknown(request, error)),
            },
            Ok(false) => match self.replay(request).await {
                Ok(Some(outcome)) => Ok(outcome),
                Ok(None) => Ok(self.conflict()),
                Err(error) => Ok(Self::unknown(request, error)),
            },
            Err(error) => Ok(Self::unknown(request, error)),
        }
    }
}
/// Ordered object publication. Receipts allow safe retries after a lost acknowledgement.
pub struct ObjectPublication {
    pub objects: ContentObjects,
    pub metadata: MetadataPublication,
}
impl ObjectPublication {
    pub async fn publish(
        &self,
        mut request: Publication,
        kind: ObjectKind,
        bytes: &[u8],
    ) -> Result<PublishOutcome> {
        let reference = ObjectRef::identify(kind, bytes);
        request.value = Some(reference.encode());
        if let Some(outcome) = self.metadata.replay(&request).await? {
            return Ok(outcome);
        }
        self.objects.put(kind, bytes).await?;
        self.metadata.publish(&request).await
    }
}
