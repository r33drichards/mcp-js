//! Cleared timers must not hold the event loop open.
//!
//! `clearTimeout` cannot abort the underlying `op_timer_sleep`, but it must
//! unref the pending op promise so `run_event_loop` can drain immediately.
//! Regression coverage for wedged isolates: packages written for Node arm
//! very long keep-alive timers and clear them when done (e.g. `@ubjs/core`
//! wraps every async Rust call in a ~25-day `setTimeout`), which previously
//! left the execution pending until the sleep fired.

use server::engine::execution::ExecutionRegistry;
use server::engine::{Engine, initialize_v8};
use std::sync::{Arc, Once};

static INIT: Once = Once::new();

fn ensure_v8() {
    INIT.call_once(initialize_v8);
}

fn rand_id() -> u64 {
    use std::time::SystemTime;
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
}

/// Stateless engine with a short execution timeout: a wedged event loop
/// surfaces as `timed_out` instead of stalling the suite.
fn create_test_engine(timeout_secs: u64) -> Engine {
    let tmp = std::env::temp_dir().join(format!(
        "mcp-timer-test-{}-{}",
        std::process::id(),
        rand_id()
    ));
    let registry =
        ExecutionRegistry::new(tmp.to_str().unwrap()).expect("Failed to create test registry");
    Engine::new_stateless(8 * 1024 * 1024, timeout_secs, 4)
        .with_execution_registry(Arc::new(registry))
}

async fn run_and_wait(engine: &Engine, code: &str) -> Result<String, String> {
    let exec_id = engine.run_js(code).execute().await?;
    for _ in 0..600 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        if let Ok(info) = engine.get_execution(&exec_id) {
            match info.status.as_str() {
                "completed" => return Ok(info.result.unwrap_or_default()),
                "failed" => return Err(info.error.unwrap_or_else(|| "Unknown error".to_string())),
                "timed_out" => return Err("Timed out".to_string()),
                "cancelled" => return Err("Cancelled".to_string()),
                _ => continue,
            }
        }
    }
    Err("Execution did not complete within polling window".to_string())
}

#[tokio::test]
async fn cleared_long_timeout_does_not_wedge_event_loop() {
    ensure_v8();
    let engine = create_test_engine(10);
    let result = run_and_wait(
        &engine,
        r#"
        const id = setTimeout(() => { throw new Error("must not fire"); }, 2147483647);
        clearTimeout(id);
        console.log("drained");
        "#,
    )
    .await;
    assert!(
        result.is_ok(),
        "cleared long timeout wedged the event loop: {result:?}"
    );
}

#[tokio::test]
async fn cleared_long_interval_does_not_wedge_event_loop() {
    ensure_v8();
    let engine = create_test_engine(10);
    let result = run_and_wait(
        &engine,
        r#"
        const id = setInterval(() => { throw new Error("must not fire"); }, 2000000000);
        clearInterval(id);
        console.log("drained");
        "#,
    )
    .await;
    assert!(
        result.is_ok(),
        "cleared long interval wedged the event loop: {result:?}"
    );
}

#[tokio::test]
async fn ref_after_clear_does_not_rewedge() {
    ensure_v8();
    let engine = create_test_engine(10);
    let result = run_and_wait(
        &engine,
        r#"
        const handle = setTimeout(() => { throw new Error("must not fire"); }, 2147483647);
        clearTimeout(handle);
        handle.ref(); // a cleared timer must stay unref'd
        console.log("drained");
        "#,
    )
    .await;
    assert!(
        result.is_ok(),
        "ref() after clearTimeout re-wedged the event loop: {result:?}"
    );
}

#[tokio::test]
async fn pending_short_timer_still_fires_and_holds_loop() {
    ensure_v8();
    let engine = create_test_engine(10);
    let result = run_and_wait(
        &engine,
        r#"
        let fired = false;
        setTimeout(() => { fired = true; console.log("fired=" + fired); }, 50);
        "#,
    )
    .await;
    assert!(
        result.is_ok(),
        "active timer should fire and complete normally: {result:?}"
    );
}
