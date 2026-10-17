use crate::{Core, Invocation, Outcome, PulseSource, Result};
use chrono::Utc;
use futures_util::FutureExt;
use std::{
    future::Future,
    panic::AssertUnwindSafe,
    pin::Pin,
    sync::{Arc, Mutex},
};
use tokio::task::JoinSet;

/// Complete only when execution has actually stopped. Report lost acknowledgements,
/// timeout without termination, and uncertain external execution as Unknown.
/// Integrators must preserve their normal execution authorization and resource limits.
pub trait Executor: Send + Sync + 'static {
    fn execute(&self, invocation: Invocation)
    -> Pin<Box<dyn Future<Output = Outcome> + Send + '_>>;
}

struct Lease(Arc<Mutex<Core>>);
impl Drop for Lease {
    fn drop(&mut self) {
        if let Ok(mut core) = self.0.lock() {
            // Cancellation/panic cannot make a consumed attempt executable again.
            let _ = core.interrupt_running();
            core.runner_active = false;
        }
    }
}

/// Run one driver per core. Shutdown stops admission, then drains executing futures.
/// There is no forced termination deadline; aborting this future makes its in-flight
/// receipts Unknown. Unknown work holds overlap/capacity slots until explicitly resolved.
/// Sled writes and flushes are synchronous; this comparison core targets small local workloads.
pub async fn run_until(
    core: Arc<Mutex<Core>>,
    executor: Arc<dyn Executor>,
    stop: impl Future<Output = ()>,
) -> Result<()> {
    {
        let mut state = core.lock().map_err(|_| "core lock poisoned")?;
        if state.runner_active {
            return Err("a runner already owns this core".into());
        }
        state.runner_active = true;
    }
    let _lease = Lease(core.clone());
    let mut source = PulseSource::start().await?;
    let mut running = JoinSet::new();
    tokio::pin!(stop);
    let mut stopping = false;
    if (&mut stop).now_or_never().is_some() {
        return source.shutdown().await;
    }
    core.lock()
        .map_err(|_| "core lock poisoned")?
        .observe(Utc::now())?;
    loop {
        if !stopping {
            loop {
                if (&mut stop).now_or_never().is_some() {
                    stopping = true;
                    break;
                }
                let invocation = core
                    .lock()
                    .map_err(|_| "core lock poisoned")?
                    .claim(Utc::now())?;
                let Some(invocation) = invocation else {
                    break;
                };
                let executor = executor.clone();
                running.spawn(async move {
                    let outcome =
                        AssertUnwindSafe(async { executor.execute(invocation.clone()).await })
                            .catch_unwind()
                            .await
                            .unwrap_or_else(|_| Outcome::Unknown("executor panicked".into()));
                    (invocation, outcome)
                });
            }
        }
        if stopping && running.is_empty() {
            break;
        }
        tokio::select! {
            biased;
            () = &mut stop, if !stopping => { stopping = true; }
            completed = running.join_next(), if !running.is_empty() => {
                let (invocation, outcome) = completed.ok_or("executor task disappeared")?
                    .map_err(|e| e.to_string())?;
                core.lock().map_err(|_| "core lock poisoned")?.finish(&invocation, outcome, Utc::now())?;
            }
            pulse = source.recv(), if !stopping => {
                pulse?;
                core.lock().map_err(|_| "core lock poisoned")?.observe(Utc::now())?;
            }
        }
    }
    source.shutdown().await
}
