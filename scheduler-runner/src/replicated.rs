use crate::cluster::{ClusterNode, Role};
use mcp_js_scheduler_core::{Core, Definition, Invocation, Limits, Outcome, Result, Time};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

const KEY: &str = "scheduler/state/v1";
#[derive(Serialize, Deserialize)]
struct Envelope {
    revision: u64,
    owner: String,
    term: u64,
    contract: String,
    checkpoint: String,
}

/// A term-scoped planner. Every mutation, including admission and completion,
/// commits through the existing Raft implementation before returning its result.
/// Dropping/losing this owner never resets consumed work to Pending.
pub struct Replicated {
    node: Arc<ClusterNode>,
    expected: String,
    envelope: Envelope,
    core: Core,
    limits: Limits,
    valid: bool,
}
impl Replicated {
    /// Must be called once for each leadership term/process incarnation.
    /// `contract` binds the operator-owned execution grant and initial definitions.
    pub async fn acquire(
        node: Arc<ClusterNode>,
        limits: Limits,
        definitions: &[Definition],
        contract: String,
        now: Time,
    ) -> Result<Self> {
        if !node.has_fixed_voters() {
            return Err("scheduler requires fixed voters".into());
        }
        let status = node.status().await;
        if status.role != Role::Leader {
            return Err("not leader".into());
        }
        node.scheduler_barrier(status.term).await?;
        let expected = node.get(KEY).await?; // Stale read is only a CAS candidate.
        let old: Option<Envelope> = expected
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(|e| e.to_string())?;
        if old.as_ref().is_some_and(|old| old.contract != contract) {
            return Err("persisted execution contract differs; explicit migration required".into());
        }
        let mut core = Core::from_checkpoint(
            old.as_ref().map(|old| old.checkpoint.as_str()),
            limits.clone(),
            true,
        )?;
        if old.is_none() {
            for definition in definitions {
                core.create(definition.clone(), now)?;
            }
        }
        let envelope = Envelope {
            revision: old
                .as_ref()
                .map_or(Some(1), |e| e.revision.checked_add(1))
                .ok_or("revision exhausted")?,
            owner: node.config.node_id.clone(),
            term: status.term,
            contract,
            checkpoint: core.checkpoint()?,
        };
        let new = serde_json::to_string(&envelope).map_err(|e| e.to_string())?;
        // Exact revision compare plus current-term durable commit is the authority.
        if !node
            .scheduler_cas(status.term, KEY.into(), expected, new.clone())
            .await?
        {
            return Err("acquisition raced; reload".into());
        }
        Ok(Self {
            node,
            expected: new,
            envelope,
            core,
            limits,
            valid: true,
        })
    }
    pub fn term(&self) -> u64 {
        self.envelope.term
    }
    pub fn view(&self) -> &Core {
        &self.core
    }
    async fn transition<T>(&mut self, action: impl FnOnce(&mut Core) -> Result<T>) -> Result<T> {
        if !self.valid {
            return Err("owner invalidated; reacquire".into());
        }
        let status = self.node.status().await;
        if status.role != Role::Leader || status.term != self.envelope.term {
            self.valid = false;
            return Err("leadership changed".into());
        }
        let mut staged =
            Core::from_checkpoint(Some(&self.envelope.checkpoint), self.limits.clone(), false)?;
        let result = action(&mut staged)?;
        let revision = self
            .envelope
            .revision
            .checked_add(1)
            .ok_or("revision exhausted")?;
        let envelope = Envelope {
            revision,
            owner: self.envelope.owner.clone(),
            term: self.envelope.term,
            contract: self.envelope.contract.clone(),
            checkpoint: staged.checkpoint()?,
        };
        let new = serde_json::to_string(&envelope).map_err(|e| e.to_string())?;
        // Set invalid BEFORE awaiting: cancellation must not leave a reusable owner.
        self.valid = false;
        if !self
            .node
            .scheduler_cas(
                self.envelope.term,
                KEY.into(),
                Some(self.expected.clone()),
                new.clone(),
            )
            .await?
        {
            return Err("ownership compare failed".into());
        }
        self.expected = new;
        self.envelope = envelope;
        self.core = staged;
        self.valid = true;
        Ok(result)
    }
    pub async fn observe(&mut self, now: Time) -> Result<()> {
        self.transition(|core| core.observe(now)).await
    }
    pub async fn claim(&mut self, now: Time) -> Result<Option<Invocation>> {
        self.transition(|core| core.claim(now)).await
    }
    pub async fn finish(
        &mut self,
        invocation: &Invocation,
        outcome: Outcome,
        now: Time,
    ) -> Result<()> {
        self.transition(|core| core.finish(invocation, outcome, now))
            .await
    }
    pub async fn create(&mut self, definition: Definition, now: Time) -> Result<u64> {
        self.transition(|core| core.create(definition, now)).await
    }
    pub async fn disable(&mut self, id: u64) -> Result<()> {
        self.transition(|core| core.disable(id)).await
    }
    pub async fn acknowledge_stopped(&mut self, id: u64) -> Result<()> {
        self.transition(|core| core.acknowledge_stopped(id)).await
    }
    pub async fn delete(&mut self, id: u64) -> Result<()> {
        self.transition(|core| core.delete(id)).await
    }
}
