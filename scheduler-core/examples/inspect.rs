//! Deterministic policy walkthrough; payloads are opaque and no JavaScript is executed.
use chrono::{DateTime, Duration};
use mcp_js_scheduler_core::*;

fn main() -> Result<()> {
    let directory = tempfile::tempdir().map_err(|e| e.to_string())?;
    let mut core = Core::open(directory.path(), Limits::default())?;
    let now = DateTime::from_timestamp(1_767_225_600, 0).unwrap();
    core.create(
        Definition {
            name: Some("comparison".into()),
            code: "console.log('scheduled check')".into(),
            cron: "* * * * *".into(),
            timezone: "UTC".into(),
            policy: Policy {
                missed: MissedRuns::CatchUp { cap: 2 },
                overlap: Overlap::Queue,
                retry: Retry {
                    max_retries: 1,
                    delay_seconds: 10,
                },
            },
        },
        now,
    )?;
    let recovered_at = now + Duration::minutes(5) + Duration::seconds(10);
    core.observe(recovered_at)?;
    while let Some(invocation) = core.claim(recovered_at)? {
        println!(
            "Inspecting occurrence {} due {} attempt {} (payload not executed)",
            invocation.occurrence_id, invocation.due_at, invocation.attempt
        );
        core.finish(&invocation, Outcome::Succeeded, recovered_at)?;
    }
    println!("{} retained receipts", core.occurrences().len());
    Ok(())
}
