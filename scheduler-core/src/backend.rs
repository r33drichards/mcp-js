use crate::Result;
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};
use tokio_cron_scheduler::{Job, JobScheduler};

/// A single native cron job wakes the durable policy core. It never executes payloads.
/// Capacity one coalesces delayed pulses rather than accumulating a callback backlog.
pub struct PulseSource {
    rx: mpsc::Receiver<()>,
    stop: Option<oneshot::Sender<()>>,
    supervisor: JoinHandle<Result<()>>,
}

impl PulseSource {
    pub async fn start() -> Result<Self> {
        let (tx, rx) = mpsc::channel(1);
        let mut scheduler = JobScheduler::new().await.map_err(|e| e.to_string())?;
        let job = Job::new_async("* * * * * *", move |_, _| {
            let tx = tx.clone();
            Box::pin(async move {
                let _ = tx.try_send(());
            })
        })
        .map_err(|e| e.to_string())?;
        if let Err(error) = scheduler.add(job).await {
            let _ = scheduler.shutdown().await;
            return Err(error.to_string());
        }
        if let Err(error) = scheduler.start().await {
            let _ = scheduler.shutdown().await;
            return Err(error.to_string());
        }
        let (stop, stopped) = oneshot::channel();
        // Dropping PulseSource closes this channel, so cancellation also shuts down.
        let supervisor = tokio::spawn(async move {
            let _ = stopped.await;
            scheduler.shutdown().await.map_err(|e| e.to_string())
        });
        Ok(Self {
            rx,
            stop: Some(stop),
            supervisor,
        })
    }

    pub async fn recv(&mut self) -> Result<()> {
        self.rx
            .recv()
            .await
            .ok_or_else(|| "tokio-cron observation job stopped".into())
    }

    pub async fn shutdown(mut self) -> Result<()> {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        (&mut self.supervisor).await.map_err(|e| e.to_string())?
    }
}

impl Drop for PulseSource {
    fn drop(&mut self) {
        self.stop.take();
    }
}
