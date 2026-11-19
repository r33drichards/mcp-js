//! Separate metadata reactor and bounded blob workers. Deadlines start at enqueue.
use crate::{MetadataStore, Result, blob::BlobStorage};
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex, OnceLock, mpsc},
    time::{Duration, Instant},
};
type MetadataJob =
    Box<dyn FnOnce(Arc<dyn MetadataStore>) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send>;
#[derive(Clone)]
pub(crate) struct MetadataWorker {
    tx: tokio::sync::mpsc::UnboundedSender<MetadataJob>,
}
pub(crate) const DEADLINE: Duration = Duration::from_secs(10);
fn remaining(deadline: Instant) -> Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .ok_or("runtime operation timed out including queue time; outcome may be uncertain".into())
}
async fn within<T>(deadline: Instant, future: impl Future<Output = Result<T>>) -> Result<T> {
    tokio::time::timeout(remaining(deadline)?, future)
        .await
        .map_err(|_| {
            "runtime operation timed out including queue time; outcome may be uncertain".to_string()
        })?
}
impl MetadataWorker {
    pub(crate) fn new<F>(factory: F) -> Result<(Self, Option<Arc<dyn BlobStorage>>)>
    where
        F: Future<Output = Result<(Arc<dyn MetadataStore>, Option<Arc<dyn BlobStorage>>)>>
            + Send
            + 'static,
    {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<MetadataJob>();
        let (ready, started) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("runtime-metadata".into())
            .spawn(move || {
                let run = || -> Result<()> {
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(|e| e.to_string())?;
                    rt.block_on(async move {
                        let (store, blobs) = match factory.await {
                            Ok(value) => value,
                            Err(e) => {
                                let _ = ready.send(Err(e));
                                return Ok(());
                            }
                        };
                        if ready.send(Ok(blobs)).is_err() {
                            return Ok(());
                        }
                        // Continuously drive the reactor, including S3 clients created
                        // by the factory, without waiting for blob requests in this queue.
                        while let Some(job) = rx.recv().await {
                            job(store.clone()).await;
                        }
                        Ok(())
                    })
                };
                let _ = run();
            })
            .map_err(|e| e.to_string())?;
        let blobs = started
            .recv()
            .map_err(|_| "runtime metadata initialization failed")??;
        Ok((Self { tx }, blobs))
    }
    pub(crate) fn call<T, F, Fut>(&self, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(Arc<dyn MetadataStore>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T>> + Send + 'static,
    {
        let deadline = Instant::now() + DEADLINE;
        let (tx, rx) = mpsc::sync_channel(1);
        self.tx
            .send(Box::new(move |store| {
                Box::pin(async move {
                    let result = match remaining(deadline) {
                        Ok(_) => within(deadline, f(store)).await,
                        Err(e) => Err(e),
                    };
                    let _ = tx.send(result);
                })
            }))
            .map_err(|_| "runtime metadata worker stopped")?;
        rx.recv_timeout(remaining(deadline)?).map_err(|_| {
            "runtime metadata deadline exceeded including queue time; outcome may be uncertain"
                .to_string()
        })?
    }
}
type BlobJob = Box<dyn FnOnce(&tokio::runtime::Runtime) + Send>;
struct Pool {
    tx: mpsc::SyncSender<BlobJob>,
}
pub(crate) struct BlobWorker {
    pub(crate) backend: Arc<dyn BlobStorage>,
    keepalive: MetadataWorker,
    concurrency: usize,
    queue: usize,
    timeout: Duration,
    pool: OnceLock<Result<Pool>>,
    bound: Mutex<Option<(Arc<dyn BlobStorage>, Option<String>)>>,
}
impl BlobWorker {
    pub(crate) fn new(backend: Arc<dyn BlobStorage>, keepalive: MetadataWorker) -> Self {
        Self {
            backend,
            keepalive,
            concurrency: 4,
            queue: 16,
            timeout: DEADLINE,
            pool: OnceLock::new(),
            bound: Mutex::new(None),
        }
    }
    pub(crate) fn limits(
        &self,
        concurrency: usize,
        queue: usize,
        timeout: Duration,
    ) -> Result<Self> {
        if concurrency == 0 || queue == 0 || timeout.is_zero() {
            return Err("blob worker limits must be positive".into());
        }
        Ok(Self {
            backend: self.backend.clone(),
            keepalive: self.keepalive.clone(),
            concurrency,
            queue,
            timeout,
            pool: OnceLock::new(),
            bound: Mutex::new(None),
        })
    }
    pub(crate) fn bound(&self) -> Result<(Arc<dyn BlobStorage>, Option<String>)> {
        let mut bound = self
            .bound
            .lock()
            .map_err(|_| "runtime scope lock poisoned")?;
        if let Some(value) = &*bound {
            return Ok(value.clone());
        }
        let value = if self.backend.runtime_gc_identity().is_some() {
            let owner = self.keepalive.call(|db| async move {
                for _ in 0..32 {
                    if let Some(id) = db.get("__runtime_owner:v1", b"id").await? {
                        let owner = String::from_utf8(id).map_err(|e| e.to_string())?;
                        uuid::Uuid::parse_str(&owner).map_err(|e| e.to_string())?;
                        db.flush()?;
                        return Ok(owner);
                    }
                    let owner = uuid::Uuid::new_v4().to_string();
                    if db
                        .compare_and_swap(
                            crate::Write::new("__runtime_owner:v1", b"id", owner.as_bytes()),
                            None,
                            vec![],
                        )
                        .await?
                    {
                        db.flush()?;
                        return Ok(owner);
                    }
                }
                Err("runtime owner allocation is busy; retry".into())
            })?;
            (self.backend.runtime_scope(&owner)?, Some(owner))
        } else {
            (self.backend.clone(), None)
        };
        *bound = Some(value.clone());
        Ok(value)
    }
    fn pool(&self) -> Result<&Pool> {
        self.pool
            .get_or_init(|| {
                let (tx, rx) = mpsc::sync_channel::<BlobJob>(self.queue);
                let rx = Arc::new(Mutex::new(rx));
                for i in 0..self.concurrency {
                    let rx = rx.clone();
                    std::thread::Builder::new()
                        .name(format!("runtime-blob-{i}"))
                        .spawn(move || {
                            let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                                .enable_all()
                                .build()
                            else {
                                return;
                            };
                            loop {
                                let job = rx.lock().unwrap().recv();
                                match job {
                                    Ok(job) => job(&rt),
                                    Err(_) => break,
                                }
                            }
                        })
                        .map_err(|e| e.to_string())?;
                }
                Ok(Pool { tx })
            })
            .as_ref()
            .map_err(Clone::clone)
    }
    pub(crate) fn call<T, F, Fut>(&self, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(Arc<dyn BlobStorage>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T>> + Send + 'static,
    {
        let deadline = Instant::now() + self.timeout;
        let (tx, rx) = mpsc::sync_channel(1);
        let backend = self.bound()?.0;
        let keepalive = self.keepalive.clone();
        self.pool()?
            .tx
            .try_send(Box::new(move |rt| {
                let _keepalive = keepalive;
                let result = match remaining(deadline) {
                    Ok(_) => rt.block_on(within(deadline, f(backend))),
                    Err(e) => Err(e),
                };
                let _ = tx.send(result);
            }))
            .map_err(|e| match e {
                mpsc::TrySendError::Full(_) => "runtime blob queue is full; retry",
                mpsc::TrySendError::Disconnected(_) => "runtime blob workers stopped",
            })?;
        rx.recv_timeout(remaining(deadline)?).map_err(|_| {
            "runtime blob deadline exceeded including queue time; outcome may be uncertain"
                .to_string()
        })?
    }
}
