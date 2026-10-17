use crate::Result;
use apalis::prelude::*;
use apalis_cron::{CronScheduler, Tick};
use std::str::FromStr;
use tokio::{sync::mpsc, task::JoinHandle};

/// Apalis drives a single observation pulse, not a queue of executable occurrences.
/// Capacity one coalesces delayed pulses; the durable core decides missed-run policy.
pub struct PulseSource {
    rx: mpsc::Receiver<()>,
    worker: JoinHandle<()>,
}

impl PulseSource {
    pub async fn start() -> Result<Self> {
        let (tx, rx) = mpsc::channel(1);
        let schedule = cron::Schedule::from_str("* * * * * *").map_err(|e| e.to_string())?;
        let worker = WorkerBuilder::new("mcp-js-scheduler-observation")
            .backend(CronScheduler::new(schedule))
            .build(move |_: Tick| {
                let tx = tx.clone();
                async move {
                    let _ = tx.try_send(());
                }
            });
        // No retry layer or persistent job backend: replay belongs to the core contract.
        let worker = tokio::spawn(async move {
            let _ = worker.run().await;
        });
        Ok(Self { rx, worker })
    }

    pub async fn recv(&mut self) -> Result<()> {
        self.rx
            .recv()
            .await
            .ok_or_else(|| "Apalis observation worker stopped".into())
    }

    pub async fn shutdown(mut self) -> Result<()> {
        self.worker.abort();
        match (&mut self.worker).await {
            Ok(()) => Ok(()),
            Err(error) if error.is_cancelled() => Ok(()),
            Err(error) => Err(error.to_string()),
        }
    }
}

impl Drop for PulseSource {
    fn drop(&mut self) {
        self.worker.abort();
    }
}
