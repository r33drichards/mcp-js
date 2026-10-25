use chrono::{Duration, TimeZone, Utc};
use mcp_js_scheduler_core::*;

#[test]
fn cancellation_keeps_capacity_until_confirmed_and_preserves_schedule() {
    let dir = tempfile::tempdir().unwrap();
    let mut core = Core::open(
        dir.path(),
        Limits {
            running: 1,
            ..Limits::default()
        },
    )
    .unwrap();
    let now = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
    let id = core
        .create(
            Definition {
                name: None,
                code: "work".into(),
                cron: "* * * * *".into(),
                timezone: "UTC".into(),
                policy: Policy::default(),
            },
            now,
        )
        .unwrap();
    core.observe(now + Duration::minutes(1)).unwrap();
    let invocation = core.claim(now + Duration::minutes(1)).unwrap().unwrap();
    core.request_cancel(invocation.occurrence_id).unwrap();
    core.request_cancel(invocation.occurrence_id).unwrap();
    core.finish(
        &invocation,
        Outcome::Unknown("worker unavailable".into()),
        now,
    )
    .unwrap();
    let checkpoint = core.checkpoint().unwrap();
    let mut recovered =
        Core::from_checkpoint(Some(&checkpoint), core.limits().clone(), true).unwrap();
    assert!(matches!(
        recovered.occurrences()[0].status,
        Status::Cancelling { .. }
    ));
    assert!(recovered.schedules()[0].enabled);
    recovered.observe(now + Duration::minutes(2)).unwrap();
    assert!(
        recovered
            .claim(now + Duration::minutes(2))
            .unwrap()
            .is_none()
    );
    recovered
        .finish(&invocation, Outcome::Cancelled, now)
        .unwrap();
    assert!(matches!(
        recovered.occurrences()[0].status,
        Status::Cancelled { .. }
    ));
    recovered.request_cancel(invocation.occurrence_id).unwrap();
    recovered.observe(now + Duration::minutes(3)).unwrap();
    assert_eq!(
        recovered
            .claim(now + Duration::minutes(3))
            .unwrap()
            .unwrap()
            .schedule_id,
        id
    );
}

#[test]
fn cancelling_pending_work_never_disables_its_schedule() {
    let dir = tempfile::tempdir().unwrap();
    let mut core = Core::open(dir.path(), Limits::default()).unwrap();
    let now = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
    let id = core
        .create(
            Definition {
                name: None,
                code: "work".into(),
                cron: "* * * * *".into(),
                timezone: "UTC".into(),
                policy: Policy::default(),
            },
            now,
        )
        .unwrap();
    core.observe(now + Duration::minutes(1)).unwrap();
    core.request_cancel(core.occurrences()[0].id).unwrap();
    assert!(core.claim(now + Duration::minutes(1)).unwrap().is_none());
    assert!(core.schedules()[0].enabled);
    core.disable(id).unwrap();
    core.request_cancel(core.occurrences()[0].id).unwrap();
    assert!(!core.schedules()[0].enabled);
}
