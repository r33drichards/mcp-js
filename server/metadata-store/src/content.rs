//! Versioned content-addressed objects over shared named-blob storage.
use crate::{Result, blob::BlobStorage};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ObjectKind {
    Artifact,
    ConsolePage,
}
impl ObjectKind {
    fn code(self) -> u8 {
        match self {
            Self::Artifact => 1,
            Self::ConsolePage => 2,
        }
    }
    fn name(self) -> &'static str {
        match self {
            Self::Artifact => "artifact",
            Self::ConsolePage => "console",
        }
    }
}
/// Version 1 hashes the unencoded payload with SHA-256. Metadata is excluded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectRef {
    pub kind: ObjectKind,
    digest: [u8; 32],
    pub size_bytes: u64,
}
impl ObjectRef {
    pub fn key(&self) -> String {
        format!(
            "runtime/v2/{}/{}",
            self.kind.name(),
            hex::encode(self.digest)
        )
    }
    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = vec![1, self.kind.code()];
        bytes.extend(self.digest);
        bytes.extend(self.size_bytes.to_be_bytes());
        bytes
    }
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != 42 || bytes[0] != 1 {
            return Err("unsupported content reference".into());
        }
        let kind = match bytes[1] {
            1 => ObjectKind::Artifact,
            2 => ObjectKind::ConsolePage,
            _ => return Err("unsupported object kind".into()),
        };
        Ok(Self {
            kind,
            digest: bytes[2..34].try_into().unwrap(),
            size_bytes: u64::from_be_bytes(bytes[34..42].try_into().unwrap()),
        })
    }
}
#[derive(Clone)]
pub struct ContentObjects {
    backend: Arc<dyn BlobStorage>,
}
impl ContentObjects {
    pub fn new(backend: Arc<dyn BlobStorage>) -> Self {
        Self { backend }
    }
    pub async fn put(&self, kind: ObjectKind, bytes: &[u8]) -> Result<ObjectRef> {
        let reference = ObjectRef {
            kind,
            digest: Sha256::digest(bytes).into(),
            size_bytes: bytes.len() as u64,
        };
        self.backend.put_immutable(&reference.key(), bytes).await?;
        Ok(reference)
    }
    pub async fn get(&self, reference: &ObjectRef) -> Result<Vec<u8>> {
        let bytes = self.backend.get(&reference.key()).await?;
        if bytes.len() as u64 != reference.size_bytes
            || <[u8; 32]>::from(Sha256::digest(&bytes)) != reference.digest
        {
            return Err("content integrity verification failed".into());
        }
        Ok(bytes)
    }
}
