//! Versioned scheduled execution. Shared verbatim with the no-V8 runner tests.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Payload {
    pub code: String,
    pub session: String,
    pub timeout_seconds: u64,
    pub heap_mb: usize,
    pub expires_at: DateTime<Utc>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Dispatch {
    pub version: u32,
    pub namespace: String,
    pub schedule_id: u64,
    pub occurrence_id: u64,
    pub attempt: u32,
    pub owner: String,
    pub epoch: u64,
    pub worker_id: String,
    pub authority: String,
    pub payload_digest: String,
    pub payload: Payload,
}
pub fn digest<T: Serialize>(value: &T) -> String {
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(value).expect("serializable protocol"))
    )
}
impl Dispatch {
    pub fn key(&self) -> String {
        // Epoch deliberately excluded: takeover cannot create a new deduplication identity.
        serde_json::to_string(&(
            &self.namespace,
            self.schedule_id,
            self.occurrence_id,
            self.attempt,
        ))
        .unwrap()
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.version != 1
            || self.namespace.is_empty()
            || self.worker_id.is_empty()
            || self.owner.is_empty()
            || self.epoch == 0
            || self.authority.is_empty()
            || self.payload_digest != digest(&self.payload)
            || self.payload.session.is_empty()
            || self.payload.timeout_seconds == 0
            || self.payload.timeout_seconds > 300
            || !(16..=4096).contains(&self.payload.heap_mb)
        {
            return Err("invalid scheduled protocol or payload binding".into());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Admission {
    pub dispatch: Dispatch,
    pub worker_incarnation: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Permit {
    pub invocation: String,
    pub epoch: u64,
    pub worker_id: String,
    pub worker_incarnation: String,
    pub payload_digest: String,
    pub authority: String,
}
impl Admission {
    pub fn permit(&self) -> Permit {
        Permit {
            invocation: self.dispatch.key(),
            epoch: self.dispatch.epoch,
            worker_id: self.dispatch.worker_id.clone(),
            worker_incarnation: self.worker_incarnation.clone(),
            payload_digest: self.dispatch.payload_digest.clone(),
            authority: self.dispatch.authority.clone(),
        }
    }
}
