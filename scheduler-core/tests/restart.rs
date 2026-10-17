mod support;
use chrono::DateTime;
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
#[test]
fn process_restart_at_each_dispatch_boundary() {
    for boundary in ["pending", "claimed", "submitted", "finished"] {
        let (dir, core, _) = setup(Policy::default());
        drop(core);
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "crash_child", "--ignored", "--nocapture"])
            .env("SCHEDULER_CRASH_STORE", dir.path())
            .env("SCHEDULER_CRASH_BOUNDARY", boundary)
            .output()
            .unwrap();
        assert_eq!(
            result.status.code(),
            Some(23),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let mut recovered = support::reopen(dir.path());
        recovered.observe(t(61)).unwrap();
        let run = recovered.claim(t(61)).unwrap();
        assert_eq!(run.is_some(), boundary == "pending");
        if boundary == "claimed" || boundary == "submitted" {
            assert!(matches!(
                recovered.occurrences()[0].status,
                Status::Unknown { .. }
            ));
        }
        if boundary == "finished" {
            assert!(matches!(
                recovered.occurrences()[0].status,
                Status::Succeeded { .. }
            ));
        }
        let effects =
            std::fs::read_to_string(dir.path().join("executor-effect")).unwrap_or_default();
        assert_eq!(
            effects,
            if boundary == "submitted" || boundary == "finished" {
                "one effect"
            } else {
                ""
            }
        );
    }
}

#[test]
#[ignore = "subprocess helper; exercised by process_restart_at_each_dispatch_boundary"]
fn crash_child() {
    let path = std::env::var("SCHEDULER_CRASH_STORE").unwrap();
    let boundary = std::env::var("SCHEDULER_CRASH_BOUNDARY").unwrap();
    let mut core = support::reopen(std::path::Path::new(&path));
    core.observe(t(60)).unwrap();
    if boundary != "pending" {
        let run = core.claim(t(60)).unwrap().unwrap();
        if boundary == "submitted" || boundary == "finished" {
            std::fs::write(
                std::path::Path::new(&path).join("executor-effect"),
                "one effect",
            )
            .unwrap();
        }
        if boundary == "finished" {
            core.finish(&run, Outcome::Succeeded, t(60)).unwrap();
        }
    }
    // Exit without dropping Core/sled: each tested boundary must already be flushed.
    std::process::exit(23);
}
