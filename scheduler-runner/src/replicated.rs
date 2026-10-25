use crate::cluster::{ClusterNode, Role};
use crate::{
    RemoteConfig,
    scheduled_protocol::{Admission, Dispatch, Payload, Permit, digest},
};
use mcp_js_scheduler_core::{Core, Definition, Invocation, Limits, Outcome, Result, Time};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc};

const KEY: &str = "scheduler/state/v1";
#[derive(Serialize, Deserialize)]
struct Envelope {
    version: u32,
    epoch: u64,
    claims: BTreeMap<String, ClaimMetadata>,
    admissions: BTreeMap<String, Permit>,
    revision: u64,
    owner: String,
    term: u64,
    contract: String,
    checkpoint: String,
}

// Wire-compatible with existing v2 claims, but code is stored only in Core.
// Old full claims deserialize safely into the compact form.
#[derive(Clone, Serialize)]
#[serde(transparent)]
struct ClaimMetadata(Dispatch);
impl<'de> Deserialize<'de> for ClaimMetadata {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        Ok(Self::new(Dispatch::deserialize(deserializer)?))
    }
}
impl ClaimMetadata {
    fn new(mut dispatch: Dispatch) -> Self {
        dispatch.payload.code = String::new();
        Self(dispatch)
    }
    fn matches(&self, dispatch: &Dispatch) -> bool {
        self.0 == Self::new(dispatch.clone()).0
    }
    fn dispatch(&self, core: &Core) -> Result<Dispatch> {
        let mut dispatch = self.0.clone();
        dispatch.payload.code = core
            .definition(dispatch.schedule_id)
            .ok_or("missing claim definition")?
            .code
            .clone();
        dispatch.validate()?;
        Ok(dispatch)
    }
}
const MAX_CHECKPOINT_BYTES: usize = 4 * 1024 * 1024;
fn checked_envelope(envelope: &Envelope) -> Result<String> {
    let json = serde_json::to_string(envelope).map_err(|e| e.to_string())?;
    if json.len() > MAX_CHECKPOINT_BYTES {
        return Err("scheduler checkpoint exceeds 4 MiB budget".into());
    }
    Ok(json)
}
fn prune_receipts(core: &Core, envelope: &mut Envelope) {
    let retained: std::collections::BTreeSet<_> = core
        .occurrences()
        .iter()
        .map(|o| {
            use mcp_js_scheduler_core::Status;
            let attempt = match o.status {
                Status::Pending { attempt, .. }
                | Status::Running { attempt }
                | Status::Cancelling { attempt }
                | Status::Cancelled { attempt }
                | Status::Succeeded { attempt }
                | Status::Failed { attempt, .. }
                | Status::Unknown { attempt, .. }
                | Status::Abandoned { attempt, .. } => attempt,
            };
            (o.id, attempt)
        })
        .collect();
    envelope
        .claims
        .retain(|_, claim| retained.contains(&(claim.0.occurrence_id, claim.0.attempt)));
    envelope
        .admissions
        .retain(|key, _| envelope.claims.contains_key(key));
}
#[derive(Debug)]
pub enum AdmissionDecision {
    Granted(Permit),
    NotLaunched,
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
            .map_err(|e| {
                format!("unsupported scheduler checkpoint; explicit migration required: {e}")
            })?;
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
        if old.as_ref().is_some_and(|e| e.version != 2) {
            return Err("explicit scheduler schema migration required".into());
        }
        let mut envelope = Envelope {
            version: 2,
            epoch: old
                .as_ref()
                .map_or(Some(1), |e| e.epoch.checked_add(1))
                .ok_or("epoch exhausted")?,
            claims: old.as_ref().map(|e| e.claims.clone()).unwrap_or_default(),
            admissions: old
                .as_ref()
                .map(|e| e.admissions.clone())
                .unwrap_or_default(),
            revision: old
                .as_ref()
                .map_or(Some(1), |e| e.revision.checked_add(1))
                .ok_or("revision exhausted")?,
            owner: node.config.node_id.clone(),
            term: status.term,
            contract,
            checkpoint: core.checkpoint()?,
        };
        prune_receipts(&core, &mut envelope);
        let new = checked_envelope(&envelope)?;
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
    pub fn is_valid(&self) -> bool {
        self.valid
    }
    pub fn term(&self) -> u64 {
        self.envelope.term
    }
    pub fn view(&self) -> &Core {
        &self.core
    }
    pub fn epoch(&self) -> u64 {
        self.envelope.epoch
    }
    async fn transition<T>(&mut self, action: impl FnOnce(&mut Core) -> Result<T>) -> Result<T> {
        self.transition_state(|core, _| action(core)).await
    }
    async fn transition_state<T>(
        &mut self,
        action: impl FnOnce(&mut Core, &mut Envelope) -> Result<T>,
    ) -> Result<T> {
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
        let mut next: Envelope = serde_json::from_str(&self.expected).map_err(|e| e.to_string())?;
        let result = action(&mut staged, &mut next)?;
        let revision = self
            .envelope
            .revision
            .checked_add(1)
            .ok_or("revision exhausted")?;
        let mut envelope = Envelope {
            version: 2,
            epoch: next.epoch,
            claims: next.claims,
            admissions: next.admissions,
            revision,
            owner: self.envelope.owner.clone(),
            term: self.envelope.term,
            contract: self.envelope.contract.clone(),
            checkpoint: staged.checkpoint()?,
        };
        prune_receipts(&staged, &mut envelope);
        let new = checked_envelope(&envelope)?;
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
    pub async fn claim_for_worker(
        &mut self,
        now: Time,
        remote: &RemoteConfig,
    ) -> Result<Option<(Invocation, Dispatch)>> {
        self.transition_state(|core, envelope| {
            let Some(invocation) = core.claim(now)? else {
                return Ok(None);
            };
            let payload = Payload {
                code: invocation.code.clone(),
                session: remote.session.clone(),
                timeout_seconds: remote.timeout_seconds,
                heap_mb: remote.heap_mb,
                expires_at: remote.expires_at,
            };
            let dispatch = Dispatch {
                version: 1,
                namespace: remote.namespace.clone(),
                schedule_id: invocation.schedule_id,
                occurrence_id: invocation.occurrence_id,
                attempt: invocation.attempt,
                owner: envelope.owner.clone(),
                epoch: envelope.epoch,
                worker_id: remote.worker_id.clone(),
                authority: digest(&envelope.contract),
                payload_digest: digest(&payload),
                payload,
            };
            dispatch.validate()?;
            if envelope.claims.contains_key(&dispatch.key())
                || envelope.admissions.contains_key(&dispatch.key())
            {
                return Err("invocation identity already consumed".into());
            }
            envelope
                .claims
                .insert(dispatch.key(), ClaimMetadata::new(dispatch.clone()));
            Ok(Some((invocation, dispatch)))
        })
        .await
    }
    pub async fn admit(&mut self, request: Admission, now: Time) -> Result<Permit> {
        self.transition_state(|core, envelope| {
            request.dispatch.validate()?;
            let running = core.occurrences().iter().any(|o| {
                o.id == request.dispatch.occurrence_id
                    && o.schedule_id == request.dispatch.schedule_id
                    && o.status
                        == mcp_js_scheduler_core::Status::Running {
                            attempt: request.dispatch.attempt,
                        }
            });
            if !running
                || request.worker_incarnation.is_empty()
                || request.dispatch.epoch != envelope.epoch
                || request.dispatch.owner != envelope.owner
                || now >= request.dispatch.payload.expires_at
                || !envelope
                    .claims
                    .get(&request.dispatch.key())
                    .is_some_and(|claim| claim.matches(&request.dispatch))
                || envelope.admissions.contains_key(&request.dispatch.key())
            {
                return Err("stale, duplicate, expired or mismatched admission".into());
            }
            let permit = request.permit();
            envelope
                .admissions
                .insert(request.dispatch.key(), permit.clone());
            Ok(permit)
        })
        .await
    }
    /// Only a committed terminal fence with no existing permit can certify
    /// "not launched". Duplicate/lost-ACK admissions stay uncertain.
    pub async fn admit_classified(
        &mut self,
        request: Admission,
        now: Time,
    ) -> Result<AdmissionDecision> {
        match self.admit(request.clone(), now).await {
            Ok(permit) => return Ok(AdmissionDecision::Granted(permit)),
            Err(error) if !self.valid => return Err(error),
            Err(_) => (),
        }
        self.transition_state(|core, envelope| {
            request.dispatch.validate()?;
            let key = request.dispatch.key();
            if request.worker_incarnation.is_empty()
                || envelope.admissions.contains_key(&key)
                || !envelope
                    .claims
                    .get(&key)
                    .is_some_and(|claim| claim.matches(&request.dispatch))
            {
                return Err("admission outcome remains uncertain or binding differs".into());
            }
            core.reject_unadmitted(request.dispatch.occurrence_id, request.dispatch.attempt)?;
            Ok(AdmissionDecision::NotLaunched)
        })
        .await
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
    pub async fn request_cancel(&mut self, id: u64) -> Result<()> {
        self.transition(|core| core.request_cancel(id)).await
    }
    /// Reconcile durable intent after restart or takeover without replaying work.
    pub fn cancellations(&self) -> Vec<Dispatch> {
        self.core
            .occurrences()
            .iter()
            .filter_map(|occurrence| {
                let mcp_js_scheduler_core::Status::Cancelling { attempt } = occurrence.status
                else {
                    return None;
                };
                self.envelope
                    .claims
                    .values()
                    .find(|dispatch| {
                        dispatch.0.occurrence_id == occurrence.id && dispatch.0.attempt == attempt
                    })
                    .and_then(|claim| claim.dispatch(&self.core).ok())
            })
            .collect()
    }
    pub async fn finish_cancellation(
        &mut self,
        dispatch: &Dispatch,
        outcome: Outcome,
        now: Time,
    ) -> Result<()> {
        if !self.core.occurrences().iter().any(|o| {
            o.id == dispatch.occurrence_id
                && o.status
                    == (mcp_js_scheduler_core::Status::Cancelling {
                        attempt: dispatch.attempt,
                    })
        }) {
            return Ok(()); // normal completion or another cancellation observation won
        }
        self.finish(
            &Invocation {
                occurrence_id: dispatch.occurrence_id,
                schedule_id: dispatch.schedule_id,
                attempt: dispatch.attempt,
                due_at: self
                    .core
                    .occurrences()
                    .iter()
                    .find(|o| o.id == dispatch.occurrence_id)
                    .unwrap()
                    .due_at,
                code: dispatch.payload.code.clone(),
            },
            outcome,
            now,
        )
        .await
    }
    pub async fn acknowledge_stopped(&mut self, id: u64) -> Result<()> {
        self.transition(|core| core.acknowledge_stopped(id)).await
    }
    pub async fn delete(&mut self, id: u64) -> Result<()> {
        self.transition(|core| core.delete(id)).await
    }
}
