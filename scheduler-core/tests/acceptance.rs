mod support;
use chrono::{DateTime, Duration};
use mcp_js_scheduler_core::*;
use tempfile::TempDir;

fn t(seconds: i64) -> Time {
    DateTime::from_timestamp(1_767_225_600 + seconds, 0).unwrap()
} // 2026-01-01 UTC
fn definition(policy: Policy) -> Definition {
    Definition {
        name: Some("check".into()),
        code: "console.log('check')".into(),
        cron: "* * * * *".into(),
        timezone: "UTC".into(),
        policy,
    }
}
fn setup(policy: Policy) -> (TempDir, Core, ScheduleId) {
    let dir = tempfile::tempdir().unwrap();
    let mut core = support::reopen(dir.path());
    let id = core.create(definition(policy), t(0)).unwrap();
    (dir, core, id)
}
fn due(core: &Core) -> Vec<i64> {
    core.occurrences()
        .iter()
        .map(|o| (o.due_at - t(0)).num_seconds())
        .collect()
}

#[test]
fn conservative_defaults_and_grace() {
    assert_eq!(
        Policy::default(),
        Policy {
            missed: MissedRuns::Skip,
            overlap: Overlap::Skip,
            retry: Retry::default()
        }
    );
    let (_dir, mut core, _) = setup(Policy::default());
    core.observe(t(66)).unwrap(); // outside the five-second grace
    assert!(core.claim(t(66)).unwrap().is_none());
    core.observe(t(125)).unwrap(); // inclusive grace
    assert_eq!(core.claim(t(125)).unwrap().unwrap().due_at, t(120));
}

#[test]
fn missed_run_policies_have_a_bounded_single_recovery_decision() {
    for (missed, expected) in [
        (MissedRuns::Skip, vec![]),
        (MissedRuns::RunOnce, vec![240]),
        (MissedRuns::CatchUp { cap: 2 }, vec![180, 240]),
    ] {
        let (_dir, mut core, _) = setup(Policy {
            missed,
            overlap: Overlap::Concurrent,
            ..Policy::default()
        });
        core.observe(t(250)).unwrap();
        assert_eq!(due(&core), expected);
        core.observe(t(251)).unwrap(); // never drain the old backlog in later ticks
        assert_eq!(due(&core), expected);
        assert_eq!(core.schedules()[0].next_due, Some(t(300)));
    }
}

#[test]
fn no_backfill_when_no_slot_was_missed() {
    let (_dir, mut core, _) = setup(Policy {
        missed: MissedRuns::RunOnce,
        ..Policy::default()
    });
    core.observe(t(59)).unwrap();
    assert!(core.occurrences().is_empty());
}

#[test]
fn duplicate_and_backward_observations_never_rearm() {
    let (_dir, mut core, _) = setup(Policy::default());
    core.observe(t(60)).unwrap();
    let first = core.claim(t(60)).unwrap().unwrap();
    core.finish(&first, Outcome::Succeeded, t(61)).unwrap();
    for now in [60, 59, 0, 61, 65] {
        core.observe(t(now)).unwrap();
    }
    assert_eq!(core.occurrences().len(), 1);
    assert!(core.claim(t(65)).unwrap().is_none());
    core.observe(t(120)).unwrap();
    assert_ne!(
        core.claim(t(120)).unwrap().unwrap().occurrence_id,
        first.occurrence_id
    );
}

#[test]
fn overlap_modes_are_independent_of_missed_runs() {
    for overlap in [Overlap::Skip, Overlap::Queue, Overlap::Concurrent] {
        let (_dir, mut core, _) = setup(Policy {
            overlap: overlap.clone(),
            ..Policy::default()
        });
        core.observe(t(60)).unwrap();
        let first = core.claim(t(60)).unwrap().unwrap();
        core.observe(t(120)).unwrap();
        if overlap == Overlap::Concurrent {
            let second = core.claim(t(120)).unwrap().unwrap();
            assert_ne!(first.occurrence_id, second.occurrence_id);
        } else {
            assert!(core.claim(t(120)).unwrap().is_none());
            core.finish(&first, Outcome::Succeeded, t(121)).unwrap();
            assert_eq!(
                core.claim(t(121)).unwrap().is_some(),
                overlap == Overlap::Queue
            );
        }
    }
}

#[test]
fn all_policy_combinations_obey_overlap_and_retry_contracts() {
    for missed in [
        MissedRuns::Skip,
        MissedRuns::RunOnce,
        MissedRuns::CatchUp { cap: 3 },
    ] {
        for overlap in [Overlap::Skip, Overlap::Queue, Overlap::Concurrent] {
            for max_retries in [0, 2] {
                let (_dir, mut core, _) = setup(Policy {
                    missed: missed.clone(),
                    overlap: overlap.clone(),
                    retry: Retry {
                        max_retries,
                        delay_seconds: 10,
                    },
                });
                core.observe(t(180)).unwrap();
                let expected =
                    if matches!(missed, MissedRuns::CatchUp { .. }) && overlap != Overlap::Skip {
                        3
                    } else {
                        1
                    };
                assert_eq!(core.occurrences().len(), expected);
                let first = core.claim(t(180)).unwrap().unwrap();
                core.finish(&first, Outcome::Failed("confirmed stop".into()), t(181))
                    .unwrap();
                let receipt = core
                    .occurrences()
                    .into_iter()
                    .find(|o| o.id == first.occurrence_id)
                    .unwrap();
                assert_eq!(
                    matches!(receipt.status, Status::Pending { .. }),
                    max_retries > 0
                );
            }
        }
    }
}

#[test]
fn retries_are_delayed_bounded_and_keep_occurrence_identity() {
    let (_dir, mut core, _) = setup(Policy {
        retry: Retry {
            max_retries: 2,
            delay_seconds: 10,
        },
        ..Policy::default()
    });
    core.observe(t(60)).unwrap();
    let mut run = core.claim(t(60)).unwrap().unwrap();
    let original = run.clone();
    for attempt in 1..=3 {
        assert_eq!(run.attempt, attempt);
        assert_eq!(run.occurrence_id, original.occurrence_id);
        core.finish(
            &run,
            Outcome::Failed("failure".into()),
            t(60 + i64::from(attempt) * 10),
        )
        .unwrap();
        assert!(
            core.claim(t(69 + i64::from(attempt) * 10))
                .unwrap()
                .is_none()
        );
        if attempt < 3 {
            run = core
                .claim(t(70 + i64::from(attempt) * 10))
                .unwrap()
                .unwrap();
        }
    }
    assert!(core.claim(t(1000)).unwrap().is_none());
    assert!(matches!(
        core.occurrences()[0].status,
        Status::Failed { attempt: 3, .. }
    ));
    assert!(core.finish(&original, Outcome::Succeeded, t(1000)).is_err());
}

#[test]
fn queue_waits_for_earlier_retry_backoff() {
    let (_dir, mut core, _) = setup(Policy {
        overlap: Overlap::Queue,
        retry: Retry {
            max_retries: 1,
            delay_seconds: 120,
        },
        ..Policy::default()
    });
    core.observe(t(60)).unwrap();
    let first = core.claim(t(60)).unwrap().unwrap();
    core.finish(&first, Outcome::Failed("stopped".into()), t(61))
        .unwrap();
    core.observe(t(120)).unwrap();
    assert!(core.claim(t(120)).unwrap().is_none());
    let retry = core.claim(t(181)).unwrap().unwrap();
    assert_eq!(retry.occurrence_id, first.occurrence_id);
    core.finish(&retry, Outcome::Succeeded, t(182)).unwrap();
    assert_eq!(core.claim(t(182)).unwrap().unwrap().due_at, t(120));
}

#[test]
fn pending_receipt_survives_restart_before_claim() {
    let (dir, mut core, _) = setup(Policy::default());
    core.observe(t(60)).unwrap();
    drop(core);
    let mut reopened = support::reopen(dir.path());
    let invocation = reopened.claim(t(70)).unwrap().unwrap();
    assert_eq!(invocation.due_at, t(60));
    assert_eq!(invocation.code, "console.log('check')");
}

#[test]
fn crash_after_claim_before_submission_is_unknown_not_replayed() {
    let (dir, mut core, _) = setup(Policy {
        retry: Retry {
            max_retries: 3,
            delay_seconds: 0,
        },
        ..Policy::default()
    });
    core.observe(t(60)).unwrap();
    let first = core.claim(t(60)).unwrap().unwrap();
    drop(core); // also represents submission followed by a lost acknowledgement
    let mut core = support::reopen(dir.path());
    core.observe(t(120)).unwrap();
    assert!(core.claim(t(120)).unwrap().is_none());
    assert!(matches!(
        core.occurrences()[0].status,
        Status::Unknown { .. }
    ));
    assert!(core.finish(&first, Outcome::Succeeded, t(120)).is_err());
    core.acknowledge_stopped(first.occurrence_id).unwrap();
    assert!(core.claim(t(121)).unwrap().is_none()); // acknowledging never retries
    core.observe(t(180)).unwrap();
    assert_eq!(core.claim(t(180)).unwrap().unwrap().due_at, t(180));
}

#[test]
fn unknown_never_retries_but_concurrent_policy_allows_later_slots() {
    let (_dir, mut core, _) = setup(Policy {
        overlap: Overlap::Concurrent,
        retry: Retry {
            max_retries: 10,
            delay_seconds: 0,
        },
        ..Policy::default()
    });
    core.observe(t(60)).unwrap();
    let first = core.claim(t(60)).unwrap().unwrap();
    core.finish(
        &first,
        Outcome::Unknown("timeout without termination".into()),
        t(61),
    )
    .unwrap();
    assert!(core.claim(t(61)).unwrap().is_none());
    core.observe(t(120)).unwrap();
    assert_eq!(core.claim(t(120)).unwrap().unwrap().attempt, 1);
}

#[test]
fn durable_completion_survives_restart_and_duplicate_observation() {
    let (dir, mut core, _) = setup(Policy::default());
    core.observe(t(60)).unwrap();
    let first = core.claim(t(60)).unwrap().unwrap();
    core.finish(&first, Outcome::Succeeded, t(61)).unwrap();
    drop(core);
    let mut core = support::reopen(dir.path());
    core.observe(t(62)).unwrap();
    assert!(core.claim(t(62)).unwrap().is_none());
    assert!(matches!(
        core.occurrences()[0].status,
        Status::Succeeded { .. }
    ));
}

#[test]
fn retry_wait_survives_restart_without_early_dispatch() {
    let (dir, mut core, _) = setup(Policy {
        retry: Retry {
            max_retries: 1,
            delay_seconds: 60,
        },
        ..Policy::default()
    });
    core.observe(t(60)).unwrap();
    let first = core.claim(t(60)).unwrap().unwrap();
    core.finish(&first, Outcome::Failed("stopped".into()), t(61))
        .unwrap();
    drop(core);
    let mut core = support::reopen(dir.path());
    assert!(core.claim(t(120)).unwrap().is_none());
    assert_eq!(core.claim(t(121)).unwrap().unwrap().attempt, 2);
}

#[test]
fn restart_missed_run_policies_match_live_clock_jump() {
    for missed in [
        MissedRuns::Skip,
        MissedRuns::RunOnce,
        MissedRuns::CatchUp { cap: 2 },
    ] {
        let policy = Policy {
            missed,
            overlap: Overlap::Concurrent,
            ..Policy::default()
        };
        let (dir, core, _) = setup(policy.clone());
        let (_other, mut live, _) = setup(policy);
        drop(core);
        let mut reopened = support::reopen(dir.path());
        for c in [&mut live, &mut reopened] {
            c.observe(t(250)).unwrap();
        }
        assert_eq!(due(&live), due(&reopened));
    }
}

#[test]
fn disable_delete_stop_new_admission_but_preserve_admitted_work() {
    for delete in [false, true] {
        let (_dir, mut core, id) = setup(Policy::default());
        core.observe(t(60)).unwrap();
        if delete {
            core.delete(id).unwrap();
        } else {
            core.disable(id).unwrap();
        }
        let admitted = core.claim(t(61)).unwrap().unwrap();
        core.finish(&admitted, Outcome::Succeeded, t(62)).unwrap();
        core.observe(t(120)).unwrap();
        assert!(core.claim(t(120)).unwrap().is_none());
        assert_eq!(core.schedules().is_empty(), delete);
    }
    let (_dir, mut core, id) = setup(Policy::default());
    core.disable(id).unwrap(); // disable wins the admission race
    core.observe(t(60)).unwrap();
    assert!(core.occurrences().is_empty());
}

#[test]
fn capacity_is_bounded_and_dropped_slots_do_not_reappear() {
    let dir = tempfile::tempdir().unwrap();
    let limits = Limits {
        outstanding: 2,
        running: 1,
        records: 2,
        ..Limits::default()
    };
    let mut core = Core::open(dir.path(), limits).unwrap();
    core.create(
        definition(Policy {
            missed: MissedRuns::RunOnce,
            overlap: Overlap::Concurrent,
            ..Policy::default()
        }),
        t(0),
    )
    .unwrap();
    core.observe(t(60)).unwrap();
    let first = core.claim(t(60)).unwrap().unwrap();
    core.observe(t(120)).unwrap();
    assert!(core.claim(t(120)).unwrap().is_none());
    core.observe(t(180)).unwrap();
    assert_eq!(core.occurrences().len(), 2);
    assert_eq!(core.schedules()[0].rejected, 1);
    core.finish(&first, Outcome::Succeeded, t(181)).unwrap();
    let second = core.claim(t(181)).unwrap().unwrap();
    assert_eq!(second.due_at, t(120));
    core.finish(&second, Outcome::Succeeded, t(182)).unwrap();
    core.observe(t(183)).unwrap();
    assert!(core.claim(t(183)).unwrap().is_none());
    core.observe(t(240)).unwrap();
    assert_eq!(core.occurrences().len(), 2); // only terminal history is evicted
    assert_eq!(core.claim(t(240)).unwrap().unwrap().due_at, t(240));
}

#[test]
fn concurrent_failures_cannot_overflow_outstanding_limit() {
    let dir = tempfile::tempdir().unwrap();
    let mut core = Core::open(
        dir.path(),
        Limits {
            outstanding: 2,
            running: 2,
            ..Limits::default()
        },
    )
    .unwrap();
    core.create(
        definition(Policy {
            missed: MissedRuns::CatchUp { cap: 2 },
            overlap: Overlap::Concurrent,
            retry: Retry {
                max_retries: 1,
                delay_seconds: 60,
            },
        }),
        t(0),
    )
    .unwrap();
    core.observe(t(120)).unwrap();
    let a = core.claim(t(120)).unwrap().unwrap();
    let b = core.claim(t(120)).unwrap().unwrap();
    core.observe(t(180)).unwrap();
    for run in [a, b] {
        core.finish(&run, Outcome::Failed("stopped".into()), t(180))
            .unwrap();
    }
    assert_eq!(core.occurrences().len(), 2);
}

#[test]
fn validates_dialect_timezone_bounds_and_exclusive_store() {
    let (dir, mut core, _) = setup(Policy::default());
    assert!(Core::open(dir.path(), Limits::default()).is_err());
    for expr in [
        "* * * * * *",
        "@daily",
        "0 0 L * *",
        "0 0 * * MON",
        "99 * * * *",
        "0/10 * * * *",
    ] {
        let mut def = definition(Policy::default());
        def.cron = expr.into();
        assert!(core.create(def, t(0)).is_err(), "{expr}");
    }
    let mut def = definition(Policy::default());
    def.timezone = "server-local".into();
    assert!(core.create(def, t(0)).is_err());
    let mut def = definition(Policy::default());
    def.code = "x".repeat(65537);
    assert!(core.create(def, t(0)).is_err());
    assert!(
        core.create(
            definition(Policy {
                missed: MissedRuns::CatchUp { cap: 0 },
                ..Policy::default()
            }),
            t(0)
        )
        .is_err()
    );
    drop(core);
    assert!(
        Core::open(
            dir.path(),
            Limits {
                running: 1,
                ..Limits::default()
            }
        )
        .is_err()
    );
}

#[test]
fn numeric_weekdays_and_dom_dow_use_standard_or_semantics() {
    let dir = tempfile::tempdir().unwrap();
    let mut core = support::reopen(dir.path());
    let mut def = definition(Policy::default());
    def.cron = "0 0 15 * 0".into(); // next Sunday is Jan 4, not Jan 15
    core.create(def, t(0)).unwrap();
    assert_eq!(core.schedules()[0].next_due, Some(t(3 * 86400)));
}

#[test]
fn timezone_dst_gap_fixed_time_moves_to_first_valid_instant() {
    let dir = tempfile::tempdir().unwrap();
    let mut core = support::reopen(dir.path());
    let mut def = definition(Policy::default());
    def.cron = "30 2 * * *".into();
    def.timezone = "America/New_York".into();
    let now = "2026-03-08T00:00:00Z".parse::<Time>().unwrap();
    core.create(def, now).unwrap();
    assert_eq!(
        core.schedules()[0].next_due,
        Some("2026-03-08T07:00:00Z".parse().unwrap())
    );
    core.observe("2026-03-08T07:00:00Z".parse().unwrap())
        .unwrap();
    assert_eq!(core.occurrences().len(), 1);
}

#[test]
fn timezone_dst_fold_fixed_time_runs_only_first_instance() {
    let dir = tempfile::tempdir().unwrap();
    let mut core = support::reopen(dir.path());
    let mut def = definition(Policy {
        overlap: Overlap::Concurrent,
        ..Policy::default()
    });
    def.cron = "30 1 * * *".into();
    def.timezone = "America/New_York".into();
    core.create(def, "2026-11-01T00:00:00Z".parse().unwrap())
        .unwrap();
    let first = "2026-11-01T05:30:00Z".parse::<Time>().unwrap();
    core.observe(first).unwrap();
    core.observe(first + Duration::hours(1)).unwrap();
    assert_eq!(
        core.occurrences()
            .iter()
            .map(|o| o.due_at)
            .collect::<Vec<_>>(),
        vec![first]
    );
}

#[test]
fn timezone_dst_fold_wildcard_runs_both_instants() {
    let dir = tempfile::tempdir().unwrap();
    let mut core = support::reopen(dir.path());
    let mut def = definition(Policy {
        overlap: Overlap::Concurrent,
        ..Policy::default()
    });
    def.cron = "* 1 * * *".into();
    def.timezone = "America/New_York".into();
    core.create(def, "2026-11-01T00:00:00Z".parse().unwrap())
        .unwrap();
    for time in ["2026-11-01T05:59:00Z", "2026-11-01T06:00:00Z"] {
        core.observe(time.parse().unwrap()).unwrap();
    }
    assert_eq!(core.occurrences().len(), 2);
}
