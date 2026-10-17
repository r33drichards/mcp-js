use chrono::{Duration as ChronoDuration, Utc};
use mcp_js_scheduler_core::*;
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{Semaphore, mpsc, oneshot};

struct ControlledExecutor {
    started: mpsc::UnboundedSender<Invocation>,
    gate: Arc<Semaphore>,
    panic: bool,
}
impl Executor for ControlledExecutor {
    fn execute(&self, run: Invocation) -> Pin<Box<dyn Future<Output = Outcome> + Send + '_>> {
        Box::pin(async move {
            self.started.send(run).unwrap();
            if self.panic {
                panic!("test executor panic");
            }
            self.gate.acquire().await.unwrap().forget();
            Outcome::Succeeded
        })
    }
}

fn core() -> (tempfile::TempDir, Arc<Mutex<Core>>) {
    let dir = tempfile::tempdir().unwrap();
    let mut core = Core::open(dir.path(), Limits::default()).unwrap();
    core.create(
        Definition {
            name: None,
            code: "opaque payload".into(),
            cron: "* * * * *".into(),
            timezone: "UTC".into(),
            policy: Policy {
                missed: MissedRuns::RunOnce,
                ..Policy::default()
            },
        },
        Utc::now() - ChronoDuration::minutes(2),
    )
    .unwrap();
    (dir, Arc::new(Mutex::new(core)))
}

#[tokio::test]
async fn real_backend_emits_a_pulse_and_shuts_down() {
    let mut source = PulseSource::start().await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), source.recv())
        .await
        .unwrap()
        .unwrap();
    source.shutdown().await.unwrap();
}

#[tokio::test]
async fn runner_uses_executor_and_drains_on_shutdown() {
    let (_dir, core) = core();
    let (started, mut rx) = mpsc::unbounded_channel();
    let gate = Arc::new(Semaphore::new(0));
    let executor = Arc::new(ControlledExecutor {
        started,
        gate: gate.clone(),
        panic: false,
    });
    let (stop, stopped) = oneshot::channel();
    let task = tokio::spawn(run_until(core.clone(), executor.clone(), async {
        let _ = stopped.await;
    }));
    let run = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(run.code, "opaque payload");
    assert_eq!(run.attempt, 1);
    assert!(
        run_until(core.clone(), executor, std::future::pending())
            .await
            .is_err()
    );
    stop.send(()).unwrap();
    gate.add_permits(1);
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(matches!(
        core.lock().unwrap().occurrences()[0].status,
        Status::Succeeded { .. }
    ));
}

#[tokio::test]
async fn cancelling_runner_preserves_unknown_and_does_not_replay() {
    let (_dir, core) = core();
    let (started, mut rx) = mpsc::unbounded_channel();
    let executor = Arc::new(ControlledExecutor {
        started,
        gate: Arc::new(Semaphore::new(0)),
        panic: false,
    });
    let task = tokio::spawn(run_until(core.clone(), executor, std::future::pending()));
    tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    let mut core = core.lock().unwrap();
    assert!(matches!(
        core.occurrences()[0].status,
        Status::Unknown { .. }
    ));
    assert!(core.claim(Utc::now()).unwrap().is_none());
}

#[tokio::test]
async fn executor_panic_is_unknown_not_a_confirmed_failure() {
    let (_dir, core) = core();
    let (started, mut rx) = mpsc::unbounded_channel();
    let executor = Arc::new(ControlledExecutor {
        started,
        gate: Arc::new(Semaphore::new(0)),
        panic: true,
    });
    let (stop, stopped) = oneshot::channel();
    let task = tokio::spawn(run_until(core.clone(), executor, async {
        let _ = stopped.await;
    }));
    tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .unwrap();
    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(matches!(
        core.lock().unwrap().occurrences()[0].status,
        Status::Unknown { .. }
    ));
}

#[tokio::test]
async fn already_requested_shutdown_does_not_admit_work() {
    let (_dir, core) = core();
    let (started, mut rx) = mpsc::unbounded_channel();
    let executor = Arc::new(ControlledExecutor {
        started,
        gate: Arc::new(Semaphore::new(0)),
        panic: false,
    });
    run_until(core.clone(), executor, std::future::ready(()))
        .await
        .unwrap();
    assert!(core.lock().unwrap().occurrences().is_empty());
    assert!(rx.try_recv().is_err());
}
