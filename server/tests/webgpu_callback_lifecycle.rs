//! Native callback lifecycle prototype. Not wired into the JS API yet.
//! Run the ignored test with a real GPU. No synthetic event is called device loss.
use std::{
    collections::VecDeque,
    fmt::{self, Write},
    sync::{Arc, Mutex},
};
const MAX_EVENTS: usize = 16;
const MAX_MESSAGE: usize = 1024;
#[derive(Debug, PartialEq)]
struct Event {
    kind: &'static str,
    message: String,
}
#[derive(Default)]
struct Events {
    errors: VecDeque<Event>,
    loss: Option<Event>,
    overflow: bool,
    closed: bool,
}
// Bound formatting itself, not merely the retained String after formatting.
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
    fn error(&mut self, error: wgpu::Error) {
        if self.closed || self.overflow {
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
    fn lost(&mut self, reason: wgpu::DeviceLostReason, text: String) {
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
    fn close(&mut self) {
        self.closed = true;
        self.errors.clear();
        self.loss = None;
    }
}
fn attach(device: &wgpu::Device, events: &Arc<Mutex<Events>>) {
    let errors = Arc::downgrade(events);
    device.on_uncaptured_error(Box::new(move |error| {
        if let Some(events) = errors.upgrade() {
            events
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .error(error);
        }
    }));
    let loss = Arc::downgrade(events);
    device.set_device_lost_callback(move |reason, text| {
        if let Some(events) = loss.upgrade() {
            events
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .lost(reason, text);
        }
    });
}
fn validation(text: String) -> wgpu::Error {
    wgpu::Error::Validation {
        source: Box::new(std::io::Error::other("test")),
        description: text,
    }
}
#[test]
fn bounded_typed_events_and_loss_survives_overflow() {
    let mut events = Events::default();
    for _ in 0..MAX_EVENTS + 10 {
        events.error(validation("é".repeat(2000)));
    }
    assert_eq!(events.errors.len(), MAX_EVENTS);
    assert!(events.overflow);
    assert!(
        events
            .errors
            .iter()
            .all(|e| e.kind == "validation" && e.message.len() <= MAX_MESSAGE)
    );
    events.lost(wgpu::DeviceLostReason::Destroyed, "first".into());
    events.lost(wgpu::DeviceLostReason::Unknown, "second".into());
    assert_eq!(events.loss.as_ref().unwrap().message, "first");
    events.close();
    events.error(validation("late".into()));
    events.lost(wgpu::DeviceLostReason::Unknown, "late".into());
    assert!(events.errors.is_empty() && events.loss.is_none());
}
#[test]
fn preserves_native_error_types() {
    let mut events = Events::default();
    events.error(wgpu::Error::OutOfMemory {
        source: Box::new(std::io::Error::other("test")),
    });
    events.error(wgpu::Error::Internal {
        source: Box::new(std::io::Error::other("test")),
        description: "internal".into(),
    });
    assert_eq!(
        events.errors.iter().map(|e| e.kind).collect::<Vec<_>>(),
        ["out-of-memory", "internal"]
    );
}
#[test]
#[ignore = "requires real GPU"]
fn real_validation_destroy_and_weak_ownership() {
    let instance = wgpu::Instance::new(&Default::default());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
    let (device, _queue) = pollster::block_on(adapter.request_device(&Default::default())).unwrap();
    let events = Arc::new(Mutex::new(Events::default()));
    attach(&device, &events);
    assert_eq!(
        Arc::strong_count(&events),
        1,
        "callbacks must not retain execution state"
    );
    let _invalid = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: None,
        source: wgpu::ShaderSource::Wgsl("not valid WGSL".into()),
    });
    device.poll(wgpu::PollType::Wait).unwrap();
    assert_eq!(
        events.lock().unwrap().errors.front().unwrap().kind,
        "validation"
    );
    device.destroy();
    let _ = device.poll(wgpu::PollType::Wait);
    assert_eq!(
        events.lock().unwrap().loss.as_ref().unwrap().kind,
        "destroyed"
    );
    let weak = Arc::downgrade(&events);
    drop(events);
    assert!(weak.upgrade().is_none());
    drop(device); // callbacks may outlive collector; no captured Session/device cycle.
}
