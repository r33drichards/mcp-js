//! Opt-in compute subset. Native handles are execution-local, never snapshotted.
use super::hooks::{HookChain, PreOutcome};
use deno_core::{OpState, op2};
use deno_error::JsErrorBox;
use serde::Deserialize;
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex, OnceLock},
};
use wgpu::util::DeviceExt;

const MAX_BYTES: usize = 4 * 1024 * 1024;
const MAX_SOURCE: usize = 64 * 1024;
const MAX_SUBMISSIONS: u32 = 32;

#[derive(Clone)]
pub struct GpuConfig {
    pub hooks: Arc<HookChain>,
}
impl GpuConfig {
    pub fn new_with_hooks(hooks: Arc<HookChain>) -> Self {
        Self { hooks }
    }
}
struct Session {
    token: String,
    device: wgpu::Device,
    queue: wgpu::Queue,
    // Error scopes are device-wide. Prevent overlapping dispatch/validation.
    busy: Mutex<()>,
    _permit: tokio::sync::OwnedSemaphorePermit,
}
impl Drop for Session {
    fn drop(&mut self) {
        self.device.destroy();
    }
}
#[derive(Default)]
struct GpuState {
    requested: bool,
    submissions: u32,
    session: Option<Arc<Session>>,
}
fn err(e: impl std::fmt::Display) -> JsErrorBox {
    JsErrorBox::generic(format!("webgpu: {e}"))
}
fn capacity() -> Result<tokio::sync::OwnedSemaphorePermit, JsErrorBox> {
    static SLOTS: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();
    SLOTS
        .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(2)))
        .clone()
        .try_acquire_owned()
        .map_err(|_| err("process GPU session limit (2)"))
}
async fn authorize(hooks: Arc<HookChain>, input: serde_json::Value) -> Result<(), JsErrorBox> {
    if hooks.has_stack() {
        hooks.run_stack_gate(input, |_| Ok(())).await.map_err(err)?;
    } else if let PreOutcome::Deny(deny) = hooks.run_pre(input).await.map_err(err)? {
        return Err(err(format!("policy denied: {deny}")));
    }
    Ok(())
}
#[op2]
#[string]
async fn op_gpu_request(state: Rc<RefCell<OpState>>) -> Result<String, JsErrorBox> {
    let hooks = state
        .borrow()
        .try_borrow::<GpuConfig>()
        .ok_or_else(|| err("disabled"))?
        .hooks
        .clone();
    authorize(hooks, serde_json::json!({"operation":"requestAdapter"})).await?;
    {
        let mut state = state.borrow_mut();
        if state.try_borrow::<GpuState>().is_none() {
            state.put(GpuState::default());
        }
        let gpu = state.borrow_mut::<GpuState>();
        if gpu.requested {
            return Err(err("one adapter request per execution"));
        }
        gpu.requested = true;
    }
    let permit = capacity()?;
    let session = tokio::task::spawn_blocking(move || {
        pollster::block_on(async {
            let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
            let adapter = match instance
                .request_adapter(&wgpu::RequestAdapterOptions::default())
                .await
            {
                Ok(adapter) => adapter,
                Err(_) => return Ok(None),
            };
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    label: Some("mcp-js compute"),
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::downlevel_defaults(),
                    memory_hints: wgpu::MemoryHints::MemoryUsage,
                    trace: wgpu::Trace::Off,
                })
                .await
                .map_err(err)?;
            Ok::<_, JsErrorBox>(Some(Session {
                token: uuid::Uuid::new_v4().to_string(),
                device,
                queue,
                busy: Mutex::new(()),
                _permit: permit,
            }))
        })
    })
    .await
    .map_err(err)??;
    let Some(session) = session else {
        return Ok(String::new());
    };
    let token = session.token.clone();
    state.borrow_mut().borrow_mut::<GpuState>().session = Some(Arc::new(session));
    Ok(token)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Job {
    code: String,
    entry: String,
    workgroups: [u32; 3],
    #[serde(deserialize_with = "bounded_buffers")]
    buffers: Vec<Vec<u8>>,
}
// Reject excessive element counts while parsing, before Vec allocation can grow.
fn bounded_buffers<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<Vec<u8>>, D::Error> {
    use serde::de::{DeserializeSeed, Error, SeqAccess, Visitor};
    struct Bytes;
    impl<'de> Visitor<'de> for Bytes {
        type Value = Vec<u8>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("bounded byte array")
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut bytes = Vec::new();
            while let Some(byte) = seq.next_element::<u8>()? {
                if bytes.len() >= MAX_BYTES {
                    return Err(A::Error::custom("buffer byte limit"));
                }
                bytes.push(byte);
            }
            Ok(bytes)
        }
    }
    impl<'de> DeserializeSeed<'de> for Bytes {
        type Value = Vec<u8>;
        fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
            d.deserialize_seq(self)
        }
    }
    struct Buffers;
    impl<'de> Visitor<'de> for Buffers {
        type Value = Vec<Vec<u8>>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("at most eight buffers")
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut buffers = Vec::new();
            let mut total = 0;
            while let Some(bytes) = seq.next_element_seed(Bytes)? {
                total += bytes.len();
                if buffers.len() >= 8 || total > MAX_BYTES {
                    return Err(A::Error::custom("buffer limit"));
                }
                buffers.push(bytes);
            }
            Ok(buffers)
        }
    }
    deserializer.deserialize_seq(Buffers)
}

impl Job {
    fn validate(&self) -> Result<(), JsErrorBox> {
        if self.code.len() > MAX_SOURCE || self.entry.len() > 256 {
            return Err(err("shader source limit"));
        }
        if self.buffers.is_empty() || self.buffers.len() > 8 {
            return Err(err("1–8 storage buffers required"));
        }
        if self
            .buffers
            .iter()
            .any(|b| b.is_empty() || b.len() % 4 != 0)
            || self.buffers.iter().map(Vec::len).sum::<usize>() > MAX_BYTES
        {
            return Err(err("buffer size limit or alignment"));
        }
        let product = self
            .workgroups
            .iter()
            .try_fold(1u64, |a, &b| a.checked_mul(b as u64));
        if self.workgroups.contains(&0) || product.is_none_or(|p| p > 65536) {
            return Err(err("workgroup limit"));
        }
        Ok(())
    }
}
#[op2]
#[string]
async fn op_gpu_dispatch(
    state: Rc<RefCell<OpState>>,
    #[string] token: String,
    #[string] json: String,
) -> Result<String, JsErrorBox> {
    if json.len() > MAX_BYTES * 4 + MAX_SOURCE * 6 + 4096 {
        return Err(err("request size limit"));
    }
    let job: Job = serde_json::from_str(&json).map_err(err)?;
    job.validate()?;
    let (session, hooks) = {
        let mut state = state.borrow_mut();
        let hooks = state
            .try_borrow::<GpuConfig>()
            .ok_or_else(|| err("disabled"))?
            .hooks
            .clone();
        let gpu = state
            .try_borrow_mut::<GpuState>()
            .ok_or_else(|| err("stale GPU handle; request a new adapter"))?;
        let session = gpu
            .session
            .as_ref()
            .filter(|s| s.token == token)
            .ok_or_else(|| err("stale or destroyed GPU handle"))?
            .clone();
        if gpu.submissions >= MAX_SUBMISSIONS {
            return Err(err("submission limit"));
        }
        gpu.submissions += 1;
        (session, hooks)
    };
    authorize(hooks, serde_json::json!({"operation":"dispatch", "bytes": job.buffers.iter().map(Vec::len).sum::<usize>(), "workgroups":job.workgroups})).await?;
    // A cancelled JS future cannot cancel a submitted GPU kernel. Keep the
    // session permit/handles alive until the bounded worker completes.
    tokio::task::spawn_blocking(move || run(&session, job))
        .await
        .map_err(err)?
}
fn checked<T>(device: &wgpu::Device, f: impl FnOnce() -> T) -> Result<T, JsErrorBox> {
    device.push_error_scope(wgpu::ErrorFilter::Internal);
    device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
    device.push_error_scope(wgpu::ErrorFilter::Validation);
    let value = f();
    let validation = pollster::block_on(device.pop_error_scope());
    let oom = pollster::block_on(device.pop_error_scope());
    let internal = pollster::block_on(device.pop_error_scope());
    if let Some(e) = validation.or(oom).or(internal) {
        return Err(err(e));
    }
    Ok(value)
}
fn run(session: &Session, job: Job) -> Result<String, JsErrorBox> {
    let _busy = session
        .busy
        .try_lock()
        .map_err(|_| err("device already dispatching"))?;
    let device = &session.device;
    let pipeline = checked(device, || {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Wgsl(job.code.into()),
        });
        device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: None,
            layout: None,
            module: &module,
            entry_point: Some(&job.entry),
            compilation_options: Default::default(),
            cache: None,
        })
    })?;
    let readbacks = checked(device, || {
        let buffers: Vec<_> = job
            .buffers
            .iter()
            .map(|data| {
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: None,
                    contents: data,
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                })
            })
            .collect();
        let entries: Vec<_> = buffers
            .iter()
            .enumerate()
            .map(|(i, b)| wgpu::BindGroupEntry {
                binding: i as u32,
                resource: b.as_entire_binding(),
            })
            .collect();
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &entries,
        });
        let readbacks: Vec<_> = job
            .buffers
            .iter()
            .map(|data| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: None,
                    size: data.len() as u64,
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            })
            .collect();
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: None,
                timestamp_writes: None,
            });
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups(job.workgroups[0], job.workgroups[1], job.workgroups[2]);
        }
        for (b, r) in buffers.iter().zip(&readbacks) {
            encoder.copy_buffer_to_buffer(b, 0, r, 0, r.size());
        }
        session.queue.submit([encoder.finish()]);
        readbacks
    })?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut output = Vec::new();
    for buffer in &readbacks {
        let (tx, rx) = std::sync::mpsc::channel();
        buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        loop {
            device.poll(wgpu::PollType::Poll).map_err(err)?;
            match rx.try_recv() {
                Ok(r) => {
                    r.map_err(err)?;
                    break;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    return Err(err("map callback lost"));
                }
                Err(_) => {}
            }
            if std::time::Instant::now() >= deadline {
                device.destroy();
                return Err(err("readback timeout; device destroyed"));
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        output.push(buffer.slice(..).get_mapped_range().to_vec());
        buffer.unmap();
    }
    serde_json::to_string(&output).map_err(err)
}
#[op2(fast)]
fn op_gpu_destroy(state: &mut OpState, #[string] token: &str) {
    if let Some(gpu) = state.try_borrow_mut::<GpuState>() {
        if gpu.session.as_ref().is_some_and(|s| s.token == token) {
            gpu.session.take();
        }
    }
}
#[op2(fast)]
fn op_gpu_check(state: &mut OpState, #[string] token: &str) -> Result<(), JsErrorBox> {
    let valid = state
        .try_borrow::<GpuState>()
        .and_then(|gpu| gpu.session.as_ref())
        .is_some_and(|s| s.token == token);
    if valid {
        Ok(())
    } else {
        Err(err("stale or destroyed GPU handle; request a new adapter"))
    }
}
deno_core::extension!(
    gpu_ext,
    ops = [
        op_gpu_request,
        op_gpu_dispatch,
        op_gpu_destroy,
        op_gpu_check
    ]
);
pub fn create_extension() -> deno_core::Extension {
    gpu_ext::init()
}
const JS: &str = include_str!("web_compat/webgpu.js");
pub fn inject(runtime: &mut deno_core::JsRuntime) -> Result<(), String> {
    runtime
        .execute_script("<webgpu>", JS.to_string())
        .map_err(|e| e.to_string())?;
    Ok(())
}
pub fn inject_snapshot(runtime: &mut deno_core::JsRuntimeForSnapshot) -> Result<(), String> {
    runtime
        .execute_script("<webgpu>", JS.to_string())
        .map_err(|e| e.to_string())?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_bounds() {
        assert!(serde_json::from_str::<Job>(r#"{"code":"","entry":"main","workgroups":[1,1,1],"buffers":[[],[],[],[],[],[],[],[],[]]}"#).is_err());
        let mut j = Job {
            code: String::new(),
            entry: "main".into(),
            workgroups: [1; 3],
            buffers: vec![vec![0; 4]],
        };
        assert!(j.validate().is_ok());
        j.workgroups = [u32::MAX; 3];
        assert!(j.validate().is_err());
        j.workgroups = [1; 3];
        j.buffers = vec![vec![0; MAX_BYTES + 4]];
        assert!(j.validate().is_err());
        j.buffers = vec![vec![0; 3]];
        assert!(j.validate().is_err());
        j.buffers = vec![vec![0; 4]; 9];
        assert!(j.validate().is_err());
        j.buffers = vec![vec![0; 4]];
        j.code = "a".repeat(MAX_SOURCE + 1);
        assert!(j.validate().is_err());
    }
}
