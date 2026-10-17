use crate::Result;
use chrono::{DateTime, Duration, Utc};
use chrono_tz::Tz;
use croner::{
    Cron,
    parser::{CronParser, Seconds, Year},
};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path};

pub type Time = DateTime<Utc>;
pub type ScheduleId = u64;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum MissedRuns {
    #[default]
    Skip,
    /// Coalesce all unobserved due slots into the newest one.
    RunOnce,
    /// Keep at most this many newest slots, in chronological order; drop the rest.
    CatchUp { cap: usize },
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum Overlap {
    #[default]
    Skip,
    Queue,
    Concurrent,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Retry {
    /// Additional attempts after a confirmed failure. Unknown outcomes never retry.
    pub max_retries: u32,
    pub delay_seconds: u32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Policy {
    pub missed: MissedRuns,
    pub overlap: Overlap,
    pub retry: Retry,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Limits {
    pub schedules: usize,
    pub records: usize,
    pub outstanding: usize,
    pub running: usize,
    /// A due slot at most this late is on time. Must be less than one minute.
    pub grace_seconds: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            schedules: 64,
            records: 4096,
            outstanding: 128,
            running: 16,
            grace_seconds: 5,
        }
    }
}

impl Limits {
    fn validate(&self) -> Result<()> {
        if self.schedules == 0
            || self.schedules > 1024
            || self.records == 0
            || self.records > 65536
            || self.outstanding == 0
            || self.outstanding > self.records
            || self.running == 0
            || self.running > self.outstanding
            || self.grace_seconds >= 60
        {
            return Err("invalid scheduler limits".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Definition {
    pub name: Option<String>,
    pub code: String,
    pub cron: String,
    pub timezone: String,
    pub policy: Policy,
}

impl Definition {
    fn parsed(&self) -> Result<(Cron, Tz)> {
        if self.code.len() > 65536
            || self.name.as_ref().is_some_and(|n| n.len() > 128)
            || self.cron.len() > 256
            || self.timezone.len() > 128
        {
            return Err("schedule payload exceeds size limit".into());
        }
        // A deliberate numeric five-field dialect. Do not inherit a backend's dialect.
        if self.cron.split_whitespace().count() != 5
            || !self
                .cron
                .chars()
                .all(|c| c.is_ascii_digit() || c.is_ascii_whitespace() || "*,-/".contains(c))
        {
            return Err("expected numeric five-field cron (* , - / supported)".into());
        }
        let cron = CronParser::builder()
            .seconds(Seconds::Disallowed)
            .year(Year::Disallowed)
            .build()
            .parse(&self.cron)
            .map_err(|e| e.to_string())?;
        let tz = self.timezone.parse::<Tz>().map_err(|e| e.to_string())?;
        if self.policy.retry.max_retries > 100 || self.policy.retry.delay_seconds > 86400 {
            return Err("retry limit exceeds 100 retries or 86400 seconds".into());
        }
        Ok((cron, tz))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Schedule {
    pub id: ScheduleId,
    pub definition: Definition,
    pub enabled: bool,
    pub deleted: bool,
    pub next_due: Option<Time>,
    /// Count of selected slots rejected by overlap or capacity; excludes old missed slots.
    pub rejected: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Status {
    Pending { attempt: u32, ready_at: Time },
    Running { attempt: u32 },
    Succeeded { attempt: u32 },
    Failed { attempt: u32, detail: String },
    Unknown { attempt: u32, detail: String },
    Abandoned { attempt: u32, detail: String },
}

impl Status {
    fn terminal(&self) -> bool {
        matches!(
            self,
            Self::Succeeded { .. } | Self::Failed { .. } | Self::Abandoned { .. }
        )
    }
    fn active(&self) -> bool {
        matches!(self, Self::Running { .. } | Self::Unknown { .. })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Occurrence {
    pub id: u64,
    pub schedule_id: ScheduleId,
    pub due_at: Time,
    pub status: Status,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invocation {
    pub occurrence_id: u64,
    pub schedule_id: ScheduleId,
    pub due_at: Time,
    pub attempt: u32,
    pub code: String,
}

/// Failed means the executor has stopped. Timeout or lost acknowledgement is Unknown.
#[derive(Clone, Debug)]
pub enum Outcome {
    Succeeded,
    Failed(String),
    Unknown(String),
}

#[derive(Clone, Serialize, Deserialize)]
struct State {
    version: u32,
    limits: Limits,
    next_id: u64,
    watermark: Option<Time>,
    schedules: BTreeMap<ScheduleId, Schedule>,
    occurrences: BTreeMap<u64, Occurrence>,
}

/// Owns an exclusively locked SQLite store. All admission transitions are serialized through &mut self.
/// Persistence errors poison this instance: reopen and recover before further work.
pub struct Core {
    db: rusqlite::Connection,
    _lock: std::fs::File,
    state: State,
    poisoned: bool,
    pub(crate) runner_active: bool,
    #[cfg(test)]
    fail_after_insert: bool,
}

impl Drop for Core {
    fn drop(&mut self) {
        // Explicit unlock matters when a concurrent process spawn briefly inherits
        // this open file description before exec closes it. All SQL is synchronous;
        // there is no pending transaction or background writer at this point.
        let _ = self._lock.unlock();
    }
}

fn next(cron: &Cron, tz: Tz, after: Time) -> Result<Time> {
    cron.find_next_occurrence(&after.with_timezone(&tz), false)
        .map(|t| t.with_timezone(&Utc))
        .map_err(|e| e.to_string())
}

fn detail(s: String) -> String {
    s.chars().take(1024).collect()
}

impl Core {
    /// Limits are stored on first creation and must match on subsequent opens.
    /// Running receipts become Unknown; they are never resubmitted on recovery.
    pub fn open(path: impl AsRef<Path>, limits: Limits) -> Result<Self> {
        limits.validate()?;
        let path = path.as_ref();
        std::fs::create_dir_all(path).map_err(|e| e.to_string())?;
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path.join("owner.lock"))
            .map_err(|e| e.to_string())?;
        lock.try_lock()
            .map_err(|e| format!("store already owned or lock unavailable: {e}"))?;
        let db =
            rusqlite::Connection::open(path.join("state.sqlite3")).map_err(|e| e.to_string())?;
        db.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=EXTRA;
            CREATE TABLE IF NOT EXISTS scheduler_state (id INTEGER PRIMARY KEY CHECK(id=1), payload BLOB NOT NULL);")
            .map_err(|e| e.to_string())?;
        let bytes: Option<Vec<u8>> = db
            .query_row("SELECT payload FROM scheduler_state WHERE id=1", [], |r| {
                r.get(0)
            })
            .optional()
            .map_err(|e| e.to_string())?;
        let state = match bytes {
            Some(bytes) => serde_json::from_slice::<State>(&bytes).map_err(|e| e.to_string())?,
            None => State {
                version: 1,
                limits: limits.clone(),
                next_id: 1,
                watermark: None,
                schedules: BTreeMap::new(),
                occurrences: BTreeMap::new(),
            },
        };
        if state.version != 1 {
            return Err("unsupported store version".into());
        }
        if serde_json::to_value(&state.limits).unwrap() != serde_json::to_value(&limits).unwrap() {
            return Err("store limits differ; use its original limits".into());
        }
        let mut core = Self {
            db,
            _lock: lock,
            state,
            poisoned: false,
            runner_active: false,
            #[cfg(test)]
            fail_after_insert: false,
        };
        let mut state = core.state.clone();
        for occurrence in state.occurrences.values_mut() {
            if let Status::Running { attempt } = occurrence.status {
                occurrence.status = Status::Unknown {
                    attempt,
                    detail: "interrupted before durable completion".into(),
                };
            }
        }
        core.commit(state)?;
        Ok(core)
    }

    pub(crate) fn interrupt_running(&mut self) -> Result<()> {
        let mut state = self.state.clone();
        for occurrence in state.occurrences.values_mut() {
            if let Status::Running { attempt } = occurrence.status {
                occurrence.status = Status::Unknown {
                    attempt,
                    detail: "runner interrupted before durable completion".into(),
                };
            }
        }
        self.commit(state)
    }

    pub fn is_poisoned(&self) -> bool {
        self.poisoned
    }

    fn commit(&mut self, state: State) -> Result<()> {
        if self.poisoned {
            return Err("store is poisoned; reopen before continuing".into());
        }
        let bytes = serde_json::to_vec(&state).map_err(|e| e.to_string())?;
        if let Err(e) = self.db.execute(
            "INSERT INTO scheduler_state(id,payload) VALUES(1,?1) ON CONFLICT(id) DO UPDATE SET payload=excluded.payload",
            [bytes],
        ) {
            self.poisoned = true;
            return Err(format!("persistence failed; no further dispatch permitted: {e}"));
        }
        #[cfg(test)]
        if self.fail_after_insert {
            self.poisoned = true;
            return Err("injected failure after commit, before application acknowledgement".into());
        }
        self.state = state;
        Ok(())
    }

    pub fn schedules(&self) -> Vec<Schedule> {
        self.state
            .schedules
            .values()
            .filter(|s| !s.deleted)
            .cloned()
            .collect()
    }
    pub fn occurrences(&self) -> Vec<Occurrence> {
        self.state.occurrences.values().cloned().collect()
    }
    pub fn limits(&self) -> &Limits {
        &self.state.limits
    }

    pub fn create(&mut self, definition: Definition, now: Time) -> Result<ScheduleId> {
        let (cron, tz) = definition.parsed()?;
        if self.state.watermark.is_some_and(|w| now < w) {
            return Err("creation precedes observed clock".into());
        }
        if let MissedRuns::CatchUp { cap } = definition.policy.missed
            && (cap == 0 || cap > self.state.limits.outstanding)
        {
            return Err("catch-up cap exceeds outstanding limit or is zero".into());
        }
        if self.state.schedules.len() >= self.state.limits.schedules {
            return Err("schedule limit reached (includes tombstones)".into());
        }
        let due = next(&cron, tz, now)?;
        let mut state = self.state.clone();
        let id = state.next_id;
        state.next_id = id.checked_add(1).ok_or("identity exhausted")?;
        state.schedules.insert(
            id,
            Schedule {
                id,
                definition,
                enabled: true,
                deleted: false,
                next_due: Some(due),
                rejected: 0,
            },
        );
        self.commit(state)?;
        Ok(id)
    }

    pub fn disable(&mut self, id: ScheduleId) -> Result<()> {
        self.stop_schedule(id, false)
    }
    pub fn delete(&mut self, id: ScheduleId) -> Result<()> {
        self.stop_schedule(id, true)
    }
    fn stop_schedule(&mut self, id: ScheduleId, delete: bool) -> Result<()> {
        let mut state = self.state.clone();
        let schedule = state.schedules.get_mut(&id).ok_or("unknown schedule")?;
        schedule.enabled = false;
        schedule.deleted |= delete;
        self.commit(state)
    }

    /// Observe wall time explicitly. Repeated/backward observations cannot rearm consumed slots.
    /// Capped catch-up selects the newest cap slots once and discards older backlog.
    pub fn observe(&mut self, now: Time) -> Result<()> {
        if self.poisoned {
            return Err("store is poisoned".into());
        }
        if self.state.watermark.is_some_and(|w| now <= w) {
            return Ok(());
        }
        let mut state = self.state.clone();
        let ids: Vec<_> = state.schedules.keys().copied().collect();
        for id in ids {
            let schedule = state.schedules[&id].clone();
            let Some(first) = schedule.next_due.filter(|due| *due <= now) else {
                continue;
            };
            let (cron, tz) = schedule.definition.parsed()?;
            state.schedules.get_mut(&id).unwrap().next_due = Some(next(&cron, tz, now)?);
            if !schedule.enabled {
                continue;
            }
            let cap = match schedule.definition.policy.missed {
                MissedRuns::CatchUp { cap } => cap,
                _ => 1,
            };
            let mut cursor = now.with_timezone(&tz);
            let mut selected = Vec::new();
            for n in 0..cap {
                let due = cron
                    .find_previous_occurrence(&cursor, n == 0)
                    .map_err(|e| e.to_string())?;
                let utc = due.with_timezone(&Utc);
                if utc < first {
                    break;
                }
                if schedule.definition.policy.missed == MissedRuns::Skip
                    && now.signed_duration_since(utc)
                        > Duration::seconds(state.limits.grace_seconds.into())
                {
                    break;
                }
                selected.push(utc);
                cursor = due;
            }
            selected.reverse();
            for due in selected {
                let occupied = state
                    .occurrences
                    .values()
                    .any(|o| o.schedule_id == id && !o.status.terminal());
                let outstanding = state
                    .occurrences
                    .values()
                    .filter(|o| !o.status.terminal())
                    .count();
                if (schedule.definition.policy.overlap == Overlap::Skip && occupied)
                    || outstanding >= state.limits.outstanding
                {
                    let schedule = state.schedules.get_mut(&id).unwrap();
                    schedule.rejected = schedule.rejected.saturating_add(1);
                    continue;
                }
                // Retain unresolved work; evict only oldest terminal receipts.
                if state.occurrences.len() >= state.limits.records {
                    let old = state
                        .occurrences
                        .iter()
                        .find(|(_, o)| o.status.terminal())
                        .map(|(id, _)| *id);
                    if let Some(old) = old {
                        state.occurrences.remove(&old);
                    } else {
                        let schedule = state.schedules.get_mut(&id).unwrap();
                        schedule.rejected = schedule.rejected.saturating_add(1);
                        continue;
                    }
                }
                let occurrence_id = state.next_id;
                state.next_id = occurrence_id.checked_add(1).ok_or("identity exhausted")?;
                state.occurrences.insert(
                    occurrence_id,
                    Occurrence {
                        id: occurrence_id,
                        schedule_id: id,
                        due_at: due,
                        status: Status::Pending {
                            attempt: 1,
                            ready_at: now,
                        },
                    },
                );
            }
        }
        state.watermark = Some(now);
        self.commit(state)
    }

    /// Persist a Running receipt BEFORE returning an invocation. The caller must attempt it once.
    /// A crash before submission intentionally loses this attempt rather than risking a duplicate.
    pub fn claim(&mut self, now: Time) -> Result<Option<Invocation>> {
        if self.poisoned {
            return Err("store is poisoned".into());
        }
        if self
            .state
            .occurrences
            .values()
            .filter(|o| o.status.active())
            .count()
            >= self.state.limits.running
        {
            return Ok(None);
        }
        let candidate = self.state.occurrences.values().find_map(|o| {
            let Status::Pending { attempt, ready_at } = o.status else {
                return None;
            };
            if ready_at > now {
                return None;
            }
            let schedule = &self.state.schedules[&o.schedule_id];
            if schedule.definition.policy.overlap != Overlap::Concurrent {
                // Include earlier queued/backoff attempts so queue order stays FIFO.
                let blocked = self.state.occurrences.values().any(|other| {
                    other.schedule_id == o.schedule_id
                        && other.id != o.id
                        && !other.status.terminal()
                        && (other.status.active() || other.id < o.id)
                });
                if blocked {
                    return None;
                }
            }
            Some((o.id, attempt))
        });
        let Some((id, attempt)) = candidate else {
            return Ok(None);
        };
        let mut state = self.state.clone();
        let occurrence = state.occurrences.get_mut(&id).unwrap();
        occurrence.status = Status::Running { attempt };
        let invocation = Invocation {
            occurrence_id: id,
            schedule_id: occurrence.schedule_id,
            due_at: occurrence.due_at,
            attempt,
            code: state.schedules[&occurrence.schedule_id]
                .definition
                .code
                .clone(),
        };
        self.commit(state)?;
        Ok(Some(invocation))
    }

    pub fn finish(&mut self, invocation: &Invocation, outcome: Outcome, now: Time) -> Result<()> {
        let mut state = self.state.clone();
        let occurrence = state
            .occurrences
            .get_mut(&invocation.occurrence_id)
            .ok_or("unknown occurrence")?;
        if occurrence.schedule_id != invocation.schedule_id
            || occurrence.due_at != invocation.due_at
            || occurrence.status
                != (Status::Running {
                    attempt: invocation.attempt,
                })
        {
            return Err("stale or mismatched execution completion".into());
        }
        let attempt = invocation.attempt;
        let retry = &state.schedules[&occurrence.schedule_id]
            .definition
            .policy
            .retry;
        occurrence.status = match outcome {
            Outcome::Succeeded => Status::Succeeded { attempt },
            Outcome::Failed(_) if attempt <= retry.max_retries => Status::Pending {
                attempt: attempt + 1,
                ready_at: now
                    .checked_add_signed(Duration::seconds(retry.delay_seconds.into()))
                    .ok_or("retry time overflow")?,
            },
            Outcome::Failed(message) => Status::Failed {
                attempt,
                detail: detail(message),
            },
            Outcome::Unknown(message) => Status::Unknown {
                attempt,
                detail: detail(message),
            },
        };
        self.commit(state)
    }

    /// Caller attests that execution has stopped. Release an unknown slot without retrying it.
    /// This does not establish whether external side effects occurred.
    pub fn acknowledge_stopped(&mut self, id: u64) -> Result<()> {
        let mut state = self.state.clone();
        let occurrence = state.occurrences.get_mut(&id).ok_or("unknown occurrence")?;
        let Status::Unknown { attempt, .. } = occurrence.status else {
            return Err("occurrence is not unknown".into());
        };
        occurrence.status = Status::Abandoned {
            attempt,
            detail: "operator attested stopped; outcome unresolved".into(),
        };
        self.commit(state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_write_prevents_dispatch_and_poisons_instance() {
        let dir = tempfile::tempdir().unwrap();
        let mut core = Core::open(dir.path(), Limits::default()).unwrap();
        let now = DateTime::from_timestamp(1_767_225_600, 0).unwrap();
        core.create(
            Definition {
                name: None,
                code: "payload".into(),
                cron: "* * * * *".into(),
                timezone: "UTC".into(),
                policy: Policy::default(),
            },
            now,
        )
        .unwrap();
        core.observe(now + Duration::minutes(1)).unwrap();
        core.db.execute_batch("PRAGMA query_only=ON").unwrap();
        assert!(core.claim(now + Duration::minutes(1)).is_err());
        assert!(core.is_poisoned());
        assert!(core.claim(now + Duration::minutes(1)).is_err());
        drop(core);
        let mut recovered = Core::open(dir.path(), Limits::default()).unwrap();
        // The claim never committed, so this pending receipt is safe to hand out.
        assert!(
            recovered
                .claim(now + Duration::minutes(1))
                .unwrap()
                .is_some()
        );
    }
    #[test]
    fn uncertain_storage_acknowledgement_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let mut core = Core::open(dir.path(), Limits::default()).unwrap();
        let now = DateTime::from_timestamp(1_767_225_600, 0).unwrap();
        core.create(
            Definition {
                name: None,
                code: "payload".into(),
                cron: "* * * * *".into(),
                timezone: "UTC".into(),
                policy: Policy::default(),
            },
            now,
        )
        .unwrap();
        core.observe(now + Duration::minutes(1)).unwrap();
        core.fail_after_insert = true;
        assert!(core.claim(now + Duration::minutes(1)).is_err());
        assert!(core.is_poisoned());
        assert!(core.claim(now + Duration::minutes(1)).is_err());
        assert!(core.observe(now + Duration::minutes(2)).is_err());
        // The injected failure models a durable write with a lost acknowledgement.
        drop(core);
        let mut recovered = Core::open(dir.path(), Limits::default()).unwrap();
        assert!(matches!(
            recovered.occurrences()[0].status,
            Status::Unknown { .. }
        ));
        assert!(
            recovered
                .claim(now + Duration::minutes(2))
                .unwrap()
                .is_none()
        );
    }
}
