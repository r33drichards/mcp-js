//! Execution-local bounded native error delivery. No JS handles in callbacks.
use std::{
    collections::VecDeque,
    fmt::{self, Write},
    sync::{Arc, Mutex},
};
const MAX_EVENTS: usize = 16;
const MAX_MESSAGE: usize = 1024;
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub(super) struct Event {
    pub(super) kind: &'static str,
    pub(super) message: String,
}
#[derive(Default)]
pub(super) struct Events {
    errors: VecDeque<Event>,
    loss: Option<Event>,
    overflow: bool,
    closed: bool,
}
// Bound retained/formatted output bytes, not upstream formatting CPU time or allocation.
struct Message(String);
impl Write for Message {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let mut n = (MAX_MESSAGE - self.0.len()).min(s.len());
        while !s.is_char_boundary(n) {
            n -= 1;
        }
        self.0.push_str(&s[..n]);
        Ok(())
    }
}
fn message(value: impl fmt::Display) -> String {
    let mut out = Message(String::with_capacity(MAX_MESSAGE));
    let _ = write!(out, "{value}");
    out.0
}
impl Events {
    pub(super) fn error(&mut self, error: wgpu::Error) {
        if self.closed || self.overflow || self.loss.is_some() {
            return;
        }
        if self.errors.len() == MAX_EVENTS {
            self.overflow = true;
            return;
        }
        let kind = match &error {
            wgpu::Error::Validation { .. } => "validation",
            wgpu::Error::OutOfMemory { .. } => "out-of-memory",
            wgpu::Error::Internal { .. } => "internal",
        };
        self.errors.push_back(Event {
            kind,
            message: message(error),
        });
    }
    pub(super) fn lost(&mut self, reason: wgpu::DeviceLostReason, text: String) {
        if self.closed || self.loss.is_some() {
            return;
        }
        self.loss = Some(Event {
            kind: if reason == wgpu::DeviceLostReason::Destroyed {
                "destroyed"
            } else {
                "unknown"
            },
            message: message(text),
        });
    }
    pub(super) fn close(&mut self) {
        self.closed = true;
        self.errors.clear();
        // Retain the authoritative terminal record until the collector is dropped.
    }
}

#[derive(Default)]
pub(super) struct Collector {
    state: Mutex<Events>,
    notify: tokio::sync::Notify,
    waiting: std::sync::atomic::AtomicBool,
}
#[derive(serde::Serialize)]
pub(super) struct Delivery {
    pub errors: Vec<Event>,
    pub loss: Option<Event>,
    pub overflow: bool,
    pub closed: bool,
}
impl Collector {
    pub fn close(&self) -> Delivery {
        // One critical section arbitrates callbacks vs explicit destruction.
        // Draining never consumes loss/overflow, so destroy sees earlier outcomes.
        let outcome = {
            let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            s.close();
            Delivery {
                errors: Vec::new(),
                loss: s.loss.clone(),
                overflow: s.overflow,
                closed: true,
            }
        };
        self.notify.notify_waiters();
        outcome
    }
    #[cfg(test)]
    pub(super) fn inject_terminal_for_test(&self, overflow: bool) {
        let mut s = self.state.lock().unwrap();
        if overflow {
            s.overflow = true;
        } else {
            s.lost(
                wgpu::DeviceLostReason::Unknown,
                "earlier native loss".into(),
            );
        }
    }
    pub fn validate_delivery(&self) -> Result<(), &'static str> {
        let s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if s.closed {
            return Err("GPU event receiver closed");
        }
        if s.overflow {
            return Err("GPU native error queue overflow; session terminated");
        }
        Ok(())
    }
    pub fn terminal(&self) -> bool {
        let s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        s.closed || s.overflow || s.loss.is_some()
    }
    pub async fn receive(&self) -> Result<Delivery, &'static str> {
        use std::sync::atomic::Ordering;
        if self.waiting.swap(true, Ordering::SeqCst) {
            return Err("GPU event receiver already pending");
        }
        struct Reset<'a>(&'a std::sync::atomic::AtomicBool);
        impl Drop for Reset<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::SeqCst);
            }
        }
        let _reset = Reset(&self.waiting);
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
                if s.closed || s.overflow || s.loss.is_some() || !s.errors.is_empty() {
                    return Ok(Delivery {
                        errors: s.errors.drain(..).collect(),
                        loss: s.loss.clone(),
                        overflow: s.overflow,
                        closed: s.closed,
                    });
                }
            }
            notified.await;
        }
    }
}
pub(super) fn attach(device: &wgpu::Device) -> Arc<Collector> {
    let events = Arc::new(Collector::default());
    let errors = Arc::downgrade(&events);
    device.on_uncaptured_error(Box::new(move |error| {
        if let Some(events) = errors.upgrade() {
            events
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .error(error);
            events.notify.notify_one();
        }
    }));
    let loss = Arc::downgrade(&events);
    device.set_device_lost_callback(move |reason, text| {
        if let Some(events) = loss.upgrade() {
            events
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .lost(reason, text);
            events.notify.notify_one();
        }
    });
    events
}

#[cfg(test)]
mod tests {
    use super::*;
    fn error() -> wgpu::Error {
        wgpu::Error::Validation {
            source: Box::new(std::io::Error::other("test")),
            description: "é".repeat(2000),
        }
    }
    #[tokio::test]
    async fn overflow_is_terminal_without_inventing_loss() {
        let c = Collector::default();
        for _ in 0..18 {
            c.state.lock().unwrap().error(error());
        }
        let d = c.receive().await.unwrap();
        assert!(d.overflow && d.loss.is_none());
        assert_eq!(d.errors.len(), 16);
        assert!(
            d.errors
                .iter()
                .all(|e| e.message.len() <= 1024 && e.kind == "validation")
        );
        assert!(c.terminal());
        assert!(c.validate_delivery().is_err());
        c.state
            .lock()
            .unwrap()
            .lost(wgpu::DeviceLostReason::Destroyed, "native".into());
        assert_eq!(c.receive().await.unwrap().loss.unwrap().kind, "destroyed");
    }
    #[tokio::test]
    async fn drained_batch_does_not_bypass_later_terminal_state() {
        let c = Collector::default();
        c.state.lock().unwrap().error(error());
        let drained = c.receive().await.unwrap();
        assert_eq!(drained.errors.len(), 1);
        for _ in 0..17 {
            c.state.lock().unwrap().error(error());
        }
        assert!(!drained.overflow);
        assert!(c.validate_delivery().is_err());
        c.close();
        assert!(c.validate_delivery().is_err());
    }
    #[tokio::test]
    async fn close_preserves_queued_and_drained_terminal_outcomes() {
        for drained in [false, true] {
            for overflow in [false, true] {
                let c = Collector::default();
                c.inject_terminal_for_test(overflow);
                if drained {
                    let _ = c.receive().await.unwrap();
                }
                let outcome = c.close();
                assert_eq!(outcome.overflow, overflow);
                assert_eq!(
                    outcome.loss.as_ref().map(|l| l.kind),
                    if overflow { None } else { Some("unknown") }
                );
                c.state
                    .lock()
                    .unwrap()
                    .lost(wgpu::DeviceLostReason::Destroyed, "late destroy".into());
                let repeated = c.close();
                assert_eq!(repeated.overflow, outcome.overflow);
                assert_eq!(repeated.loss, outcome.loss);
            }
        }
    }
    #[test]
    fn first_terminal_outcome_wins_before_close() {
        let loss_first = Collector::default();
        loss_first.inject_terminal_for_test(false);
        for _ in 0..18 {
            loss_first.state.lock().unwrap().error(error());
        }
        let outcome = loss_first.close();
        assert!(!outcome.overflow);
        assert_eq!(outcome.loss.unwrap().kind, "unknown");
        let overflow_first = Collector::default();
        for _ in 0..18 {
            overflow_first.state.lock().unwrap().error(error());
        }
        overflow_first
            .state
            .lock()
            .unwrap()
            .lost(wgpu::DeviceLostReason::Unknown, "later loss".into());
        let outcome = overflow_first.close();
        assert!(outcome.overflow);
        assert_eq!(outcome.loss.unwrap().kind, "unknown");
        // Both records may be diagnostic data, but overflow is the earlier outcome.
    }
    #[tokio::test]
    async fn close_wakes_receiver_and_rejects_late_callbacks() {
        let c = Arc::new(Collector::default());
        let waiter = c.clone();
        let task = tokio::spawn(async move { waiter.receive().await.unwrap() });
        tokio::task::yield_now().await;
        assert!(c.receive().await.is_err());
        c.close();
        let d = tokio::time::timeout(std::time::Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap();
        assert!(d.closed);
        assert!(c.validate_delivery().is_err());
        c.state.lock().unwrap().error(error());
        c.state
            .lock()
            .unwrap()
            .lost(wgpu::DeviceLostReason::Unknown, "late".into());
        let d = c.receive().await.unwrap();
        assert!(d.errors.is_empty() && d.loss.is_none());
    }
    #[tokio::test]
    async fn cancelled_receiver_releases_single_waiter() {
        let c = Arc::new(Collector::default());
        let waiter = c.clone();
        let task = tokio::spawn(async move { waiter.receive().await });
        tokio::task::yield_now().await;
        task.abort();
        let _ = task.await;
        c.close();
        assert!(c.receive().await.unwrap().closed);
    }
    #[test]
    #[ignore = "requires real GPU"]
    fn native_callback_scope_and_destruction() {
        let instance = wgpu::Instance::new(&Default::default());
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
        let (device, _queue) =
            pollster::block_on(adapter.request_device(&Default::default())).unwrap();
        let c = attach(&device);
        assert_eq!(Arc::strong_count(&c), 1);
        let bad = || {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: None,
                source: wgpu::ShaderSource::Wgsl("invalid WGSL".into()),
            })
        };
        assert!(super::super::checked(&device, bad).is_err());
        assert!(
            c.state.lock().unwrap().errors.is_empty(),
            "captured errors must not be delivered twice"
        );
        let _ = bad();
        device.poll(wgpu::PollType::Wait).unwrap();
        assert_eq!(
            c.state
                .lock()
                .unwrap()
                .errors
                .front()
                .expect("native uncaptured error callback")
                .kind,
            "validation"
        );
        device.destroy();
        let _ = device.poll(wgpu::PollType::Wait);
        assert_eq!(
            c.state
                .lock()
                .unwrap()
                .loss
                .as_ref()
                .expect("native device destruction callback")
                .kind,
            "destroyed"
        );
        let weak = Arc::downgrade(&c);
        drop(c);
        assert!(weak.upgrade().is_none());
    }
}
