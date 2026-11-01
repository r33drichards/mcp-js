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

#[path = "webgpu_events.rs"]
mod events;

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
pub(super) struct Session {
    token: String,
    pub(super) device: wgpu::Device,
    pub(super) queue: wgpu::Queue,
    // Error scopes are device-wide. Prevent overlapping dispatch/validation.
    pub(super) busy: Mutex<()>,
    resources: Mutex<ResourceRegistry>,
    info: serde_json::Value,
    events: Arc<events::Collector>,
    _permit: tokio::sync::OwnedSemaphorePermit,
}
impl Drop for Session {
    fn drop(&mut self) {
        self.events.close();
        self.device.destroy();
    }
}
// IDs are scoped by the native session token, never global or snapshotted.
#[derive(Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BufferRef {
    pub id: u32,
    pub revision: u32,
}
#[derive(Default)]
struct ResourceRegistry {
    entries: std::collections::HashMap<u32, ResidentBuffer>,
    bytes: usize,
}
struct ResidentBuffer {
    buffer: wgpu::Buffer,
    revision: u32,
    usage: u32,
}
pub(super) fn validate_refs(refs: &[BufferRef], count: usize) -> Result<(), JsErrorBox> {
    if refs.is_empty() {
        return Ok(());
    } // Legacy serialized jobs remain supported.
    let mut seen = std::collections::HashSet::new();
    if refs.len() != count
        || refs
            .iter()
            .any(|r| r.id == 0 || r.id > 128 || !seen.insert(r.id))
    {
        return Err(err("invalid or duplicate buffer identities"));
    }
    Ok(())
}
pub(super) fn validate_buffer_usages(data: &[Vec<u8>], usages: &[u32]) -> Result<(), JsErrorBox> {
    if data.len() != usages.len()
        || data.len() > 128
        || data.iter().map(Vec::len).sum::<usize>() > MAX_BYTES
    {
        return Err(err("buffer usage count/byte limit"));
    }
    for (bytes, &usage) in data.iter().zip(usages) {
        if bytes.is_empty()
            || bytes.len() % 4 != 0
            || usage == 0
            || usage & !237 != 0
            || (usage & 1 != 0 && usage & !9 != 0)
            || (usage & 64 != 0 && (usage & 128 != 0 || bytes.len() > 65536))
        {
            return Err(err("buffer usage/size/alignment"));
        }
    }
    Ok(())
}
// Caller holds session.busy. A CPU upload revision changes only on write/copy;
// shader writes retain identity and remain authoritative on the GPU.
pub(super) fn resident_buffers(
    session: &Session,
    data: &[Vec<u8>],
    usages: &[u32],
    refs: &[BufferRef],
) -> Result<Vec<wgpu::Buffer>, JsErrorBox> {
    validate_refs(refs, data.len())?;
    validate_buffer_usages(data, usages)?;
    let mut registry = session
        .resources
        .lock()
        .map_err(|_| err("resource registry poisoned"))?;
    let mut added = 0usize;
    for (i, r) in refs.iter().enumerate() {
        if let Some(old) = registry.entries.get(&r.id) {
            if old.buffer.size() != data[i].len() as u64
                || old.usage != usages[i]
                || r.revision < old.revision
            {
                return Err(err("buffer identity size/usage/revision mismatch"));
            }
        } else {
            added = added
                .checked_add(data[i].len())
                .ok_or_else(|| err("resource size overflow"))?;
        }
    }
    if registry.bytes + added > MAX_BYTES {
        return Err(err("resident buffer limit (4 MiB)"));
    }
    let mut result = Vec::new();
    for (i, bytes) in data.iter().enumerate() {
        let usage = usages[i];
        if let Some(r) = refs.get(i) {
            if let Some(old) = registry.entries.get_mut(&r.id) {
                if r.revision > old.revision {
                    checked(&session.device, || {
                        session.queue.write_buffer(&old.buffer, 0, bytes)
                    })?;
                    old.revision = r.revision;
                }
                result.push(old.buffer.clone());
                continue;
            }
        }
        let buffer = checked(&session.device, || {
            session
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: None,
                    contents: bytes,
                    // Mapping is backed by separate staging/readback buffers.
                    // Retain every declared GPU role, including UNIFORM | VERTEX.
                    usage: wgpu::BufferUsages::from_bits_truncate(usage & !1)
                        | wgpu::BufferUsages::COPY_SRC
                        | wgpu::BufferUsages::COPY_DST,
                })
        })?;
        if let Some(r) = refs.get(i) {
            registry.bytes += bytes.len();
            registry.entries.insert(
                r.id,
                ResidentBuffer {
                    buffer: buffer.clone(),
                    revision: r.revision,
                    usage,
                },
            );
        }
        result.push(buffer);
    }
    Ok(result)
}
#[derive(Default)]
struct GpuState {
    requested: bool,
    submissions: u32,
    session: Option<Arc<Session>>,
}
impl Drop for GpuState {
    fn drop(&mut self) {
        if let Some(session) = &self.session {
            session.events.close();
            session.device.destroy();
        }
    }
}
pub(super) fn err(e: impl std::fmt::Display) -> JsErrorBox {
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
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AdapterRequest {
    #[serde(default)]
    power_preference: Option<String>,
    #[serde(default)]
    force_fallback_adapter: bool,
}
impl AdapterRequest {
    fn preference(&self) -> Result<wgpu::PowerPreference, JsErrorBox> {
        match self.power_preference.as_deref() {
            None => Ok(Default::default()),
            Some("low-power") => Ok(wgpu::PowerPreference::LowPower),
            Some("high-performance") => Ok(wgpu::PowerPreference::HighPerformance),
            _ => Err(err("invalid powerPreference")),
        }
    }
}
#[op2]
#[string]
async fn op_gpu_request(
    state: Rc<RefCell<OpState>>,
    #[string] json: String,
) -> Result<String, JsErrorBox> {
    if json.len() > 1024 {
        return Err(err("adapter descriptor limit"));
    }
    let request: AdapterRequest = serde_json::from_str(&json).map_err(err)?;
    let preference = request.preference()?;
    let hooks = state
        .borrow()
        .try_borrow::<GpuConfig>()
        .ok_or_else(|| err("disabled"))?
        .hooks
        .clone();
    authorize(hooks, serde_json::json!({"operation":"requestAdapter","powerPreference":request.power_preference,"forceFallbackAdapter":request.force_fallback_adapter})).await?;
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
                .request_adapter(&wgpu::RequestAdapterOptions {power_preference:preference,force_fallback_adapter:request.force_fallback_adapter,..Default::default()})
                .await
            {
                Ok(adapter) => adapter,
                Err(_) => return Ok(None),
            };
            let native_info=adapter.get_info();
            let info=serde_json::json!({"vendor":format!("0x{:04x}",native_info.vendor),"device":format!("0x{:04x}",native_info.device),"architecture":"","description":native_info.name});
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
            let events=events::attach(&device);
            Ok::<_, JsErrorBox>(Some(Session {
                token: uuid::Uuid::new_v4().to_string(),
                device,
                queue,
                busy: Mutex::new(()),
                resources: Mutex::new(ResourceRegistry::default()),
                info,
                events,
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
    #[serde(default)]
    uniforms: Vec<bool>,
    #[serde(default)]
    refs: Vec<BufferRef>,
}
// Reject excessive element counts while parsing, before Vec allocation can grow.
fn bounded_buffers<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<Vec<u8>>, D::Error> {
    bounded_buffer_list(deserializer, 8)
}
pub(super) fn batch_buffers<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<Vec<u8>>, D::Error> {
    bounded_buffer_list(deserializer, 16)
}
fn bounded_buffer_list<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
    limit: usize,
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
    struct Buffers(usize);
    impl<'de> Visitor<'de> for Buffers {
        type Value = Vec<Vec<u8>>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(f, "at most {} buffers", self.0)
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut buffers = Vec::new();
            let mut total = 0;
            while let Some(bytes) = seq.next_element_seed(Bytes)? {
                total += bytes.len();
                if buffers.len() >= self.0 || total > MAX_BYTES {
                    return Err(A::Error::custom("buffer limit"));
                }
                buffers.push(bytes);
            }
            Ok(buffers)
        }
    }
    deserializer.deserialize_seq(Buffers(limit))
}

impl Job {
    fn validate(&self) -> Result<(), JsErrorBox> {
        validate_refs(&self.refs, self.buffers.len())?;
        if !self.uniforms.is_empty() && self.uniforms.len() != self.buffers.len() {
            return Err(err("binding kinds mismatch"));
        }
        if self
            .buffers
            .iter()
            .enumerate()
            .any(|(i, b)| self.uniforms.get(i) == Some(&true) && b.len() > 65536)
        {
            return Err(err("uniform buffer limit"));
        }
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
            .filter(|s| s.token == token && !s.events.terminal())
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
    tokio::task::spawn_blocking(move || {
        if session.events.terminal() {
            return Err(err("GPU session terminated"));
        }
        run(&session, job)
    })
    .await
    .map_err(err)?
}
#[op2]
#[string]
async fn op_gpu_batch(
    state: Rc<RefCell<OpState>>,
    #[string] token: String,
    #[string] json: String,
) -> Result<String, JsErrorBox> {
    if json.len() > MAX_BYTES * 4 + MAX_SOURCE * 6 + 16384 {
        return Err(err("batch request size limit"));
    }
    let job: super::webgpu_commands::Batch = serde_json::from_str(&json).map_err(err)?;
    let units = job.validate()?;
    let (session, hooks) = {
        let mut state = state.borrow_mut();
        let hooks = state
            .try_borrow::<GpuConfig>()
            .ok_or_else(|| err("disabled"))?
            .hooks
            .clone();
        let gpu = state
            .try_borrow_mut::<GpuState>()
            .ok_or_else(|| err("stale GPU handle"))?;
        let session = gpu
            .session
            .as_ref()
            .filter(|s| s.token == token && !s.events.terminal())
            .ok_or_else(|| err("stale or destroyed GPU handle"))?
            .clone();
        if units > MAX_SUBMISSIONS - gpu.submissions {
            return Err(err("GPU work limit (32)"));
        }
        gpu.submissions += units;
        (session, hooks)
    };
    // Check every operation before any upload, shader compilation or GPU work.
    for input in job.policy_inputs() {
        authorize(hooks.clone(), input).await?;
    }
    tokio::task::spawn_blocking(move || {
        if session.events.terminal() {
            return Err(err("GPU session terminated"));
        }
        super::webgpu_commands::run(&session, job)
    })
    .await
    .map_err(err)?
}
#[op2]
#[string]
async fn op_gpu_render(
    state: Rc<RefCell<OpState>>,
    #[string] token: String,
    #[string] json: String,
) -> Result<String, JsErrorBox> {
    if json.len() > 1024 * 1024 {
        return Err(err("render request size limit"));
    }
    let job: super::webgpu_render::RenderJob = serde_json::from_str(&json).map_err(err)?;
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
            .filter(|s| s.token == token && !s.events.terminal())
            .ok_or_else(|| err("stale or destroyed GPU handle"))?
            .clone();
        if gpu.submissions >= MAX_SUBMISSIONS {
            return Err(err("submission limit"));
        }
        gpu.submissions += 1;
        (session, hooks)
    };
    authorize(hooks, serde_json::json!({"operation":"render","width":job.width,"height":job.height,"bytes":job.bytes(),"draw":job.draw})).await?;
    tokio::task::spawn_blocking(move || {
        if session.events.terminal() {
            return Err(err("GPU session terminated"));
        }
        super::webgpu_render::render(&session, job)
    })
    .await
    .map_err(err)?
}

pub(super) fn checked<T>(device: &wgpu::Device, f: impl FnOnce() -> T) -> Result<T, JsErrorBox> {
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
    let usages: Vec<_> = (0..job.buffers.len())
        .map(|i| {
            if job.uniforms.get(i) == Some(&true) {
                76
            } else {
                140
            }
        })
        .collect();
    let buffers = resident_buffers(session, &job.buffers, &usages, &job.refs)?;
    let readbacks = checked(device, || {
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
    read_buffers(session, &readbacks)
}
pub(super) fn read_buffers(
    session: &Session,
    readbacks: &[wgpu::Buffer],
) -> Result<String, JsErrorBox> {
    let device = &session.device;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut output = Vec::new();
    for buffer in readbacks {
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
fn native_limits(l: &wgpu::Limits) -> serde_json::Value {
    serde_json::json!({"maxTextureDimension1D":l.max_texture_dimension_1d,"maxTextureDimension2D":l.max_texture_dimension_2d,"maxTextureDimension3D":l.max_texture_dimension_3d,"maxTextureArrayLayers":l.max_texture_array_layers,"maxBindGroups":l.max_bind_groups,"maxBindingsPerBindGroup":l.max_bindings_per_bind_group,"maxDynamicUniformBuffersPerPipelineLayout":l.max_dynamic_uniform_buffers_per_pipeline_layout,"maxDynamicStorageBuffersPerPipelineLayout":l.max_dynamic_storage_buffers_per_pipeline_layout,"maxSampledTexturesPerShaderStage":l.max_sampled_textures_per_shader_stage,"maxSamplersPerShaderStage":l.max_samplers_per_shader_stage,"maxStorageBuffersPerShaderStage":l.max_storage_buffers_per_shader_stage,"maxStorageTexturesPerShaderStage":l.max_storage_textures_per_shader_stage,"maxUniformBuffersPerShaderStage":l.max_uniform_buffers_per_shader_stage,"maxUniformBufferBindingSize":l.max_uniform_buffer_binding_size,"maxStorageBufferBindingSize":l.max_storage_buffer_binding_size,"maxVertexBuffers":l.max_vertex_buffers,"maxBufferSize":l.max_buffer_size,"maxVertexAttributes":l.max_vertex_attributes,"maxVertexBufferArrayStride":l.max_vertex_buffer_array_stride,"minUniformBufferOffsetAlignment":l.min_uniform_buffer_offset_alignment,"minStorageBufferOffsetAlignment":l.min_storage_buffer_offset_alignment,"maxInterStageShaderComponents":l.max_inter_stage_shader_components,"maxColorAttachments":l.max_color_attachments,"maxColorAttachmentBytesPerSample":l.max_color_attachment_bytes_per_sample,"maxComputeWorkgroupStorageSize":l.max_compute_workgroup_storage_size,"maxComputeInvocationsPerWorkgroup":l.max_compute_invocations_per_workgroup,"maxComputeWorkgroupSizeX":l.max_compute_workgroup_size_x,"maxComputeWorkgroupSizeY":l.max_compute_workgroup_size_y,"maxComputeWorkgroupSizeZ":l.max_compute_workgroup_size_z,"maxComputeWorkgroupsPerDimension":l.max_compute_workgroups_per_dimension})
}

// Standard fields describe the implemented subset, not inaccessible native capacity.
// Per-object maxima still coexist with cumulative/session/work budgets.
fn effective_limits(l: &wgpu::Limits) -> serde_json::Value {
    let mut result = native_limits(l);
    for (name, ceiling) in [
        ("maxTextureDimension1D", 0u64),
        ("maxTextureDimension2D", 1024),
        ("maxTextureDimension3D", 0),
        ("maxTextureArrayLayers", 1),
        ("maxBindGroups", 1),
        ("maxBindingsPerBindGroup", 8),
        ("maxDynamicUniformBuffersPerPipelineLayout", 0),
        ("maxDynamicStorageBuffersPerPipelineLayout", 0),
        ("maxSampledTexturesPerShaderStage", 0),
        ("maxSamplersPerShaderStage", 0),
        ("maxStorageBuffersPerShaderStage", 8),
        ("maxStorageTexturesPerShaderStage", 0),
        ("maxUniformBuffersPerShaderStage", 8),
        ("maxUniformBufferBindingSize", 65536),
        ("maxStorageBufferBindingSize", MAX_BYTES as u64),
        ("maxBufferSize", MAX_BYTES as u64),
        ("maxVertexBuffers", 4),
        ("maxVertexAttributes", 16),
        ("maxVertexBufferArrayStride", 2048),
        ("maxColorAttachments", 1),
        ("maxColorAttachmentBytesPerSample", 4),
        ("maxComputeWorkgroupsPerDimension", 65536),
    ] {
        let native = result[name].as_u64().expect("known native limit");
        result[name] = native.min(ceiling).into();
    }
    // The shim requires 256-byte binding offsets even on less restrictive devices.
    for name in [
        "minUniformBufferOffsetAlignment",
        "minStorageBufferOffsetAlignment",
    ] {
        result[name] = result[name].as_u64().unwrap().max(256).into();
    }
    result
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DeviceRequest {
    #[serde(default)]
    required_features: Vec<String>,
    #[serde(default)]
    required_limits: std::collections::BTreeMap<String, u64>,
}
fn validate_device_request(
    request: &DeviceRequest,
    limits: &serde_json::Value,
) -> Result<(), JsErrorBox> {
    if !request.required_features.is_empty() {
        return Err(err("optional GPU features unsupported"));
    }
    for (name, value) in &request.required_limits {
        if limits.get(name).and_then(serde_json::Value::as_u64) != Some(*value) {
            return Err(err(format!(
                "only the existing effective limit profile is supported: {name}"
            )));
        }
    }
    Ok(())
}
fn session_for_token(state: &OpState, token: &str) -> Result<Arc<Session>, JsErrorBox> {
    state
        .try_borrow::<GpuConfig>()
        .ok_or_else(|| err("disabled"))?;
    state
        .try_borrow::<GpuState>()
        .and_then(|gpu| gpu.session.as_ref())
        .filter(|s| s.token == token)
        .cloned()
        .ok_or_else(|| err("stale or destroyed GPU handle"))
}
#[op2]
#[string]
fn op_gpu_describe(state: &mut OpState, #[string] token: &str) -> Result<String, JsErrorBox> {
    let session = session_for_token(state, token)?;
    // This implementation creates a fixed, feature-empty native device during
    // adapter acquisition. Never advertise optional features enabled by neither
    // that device nor the JS subset.
    if !session.device.features().is_empty() {
        return Err(err("unexpected native feature profile"));
    }
    serde_json::to_string(&serde_json::json!({"limits":effective_limits(&session.device.limits()),"features":[],"info":session.info})).map_err(err)
}
#[op2(fast)]
fn op_gpu_validate_device(
    state: &mut OpState,
    #[string] token: &str,
    #[string] json: &str,
) -> Result<(), JsErrorBox> {
    let session = session_for_token(state, token)?;
    if json.len() > 16384 {
        return Err(err("device descriptor limit"));
    }
    let request: DeviceRequest = serde_json::from_str(json).map_err(err)?;
    validate_device_request(&request, &effective_limits(&session.device.limits()))
}
#[op2]
#[string]
async fn op_gpu_events(
    state: Rc<RefCell<OpState>>,
    #[string] token: String,
) -> Result<String, JsErrorBox> {
    // Do not retain Session while waiting: execution teardown must close/wake it.
    let events = session_for_token(&state.borrow(), &token)?.events.clone();
    let delivery = events.receive().await.map_err(err)?;
    // Validate again AFTER draining, before crossing back into JS.
    session_for_token(&state.borrow(), &token)?;
    serde_json::to_string(&delivery).map_err(err)
}
#[op2(fast)]
fn op_gpu_event_check(state: &mut OpState, #[string] token: &str) -> Result<(), JsErrorBox> {
    session_for_token(state, token)?
        .events
        .validate_delivery()
        .map_err(err)
}
#[op2]
#[string]
fn op_gpu_destroy(state: &mut OpState, #[string] token: &str) -> Result<String, JsErrorBox> {
    session_for_token(state, token)?;
    let session = state
        .borrow_mut::<GpuState>()
        .session
        .take()
        .ok_or_else(|| err("stale GPU handle"))?;
    let outcome = session.events.close();
    session.device.destroy();
    serde_json::to_string(&outcome).map_err(err)
}
#[op2(fast)]
fn op_gpu_check(state: &mut OpState, #[string] token: &str) -> Result<(), JsErrorBox> {
    let valid = state
        .try_borrow::<GpuState>()
        .and_then(|gpu| gpu.session.as_ref())
        .is_some_and(|s| s.token == token && !s.events.terminal());
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
        op_gpu_check,
        op_gpu_render,
        op_gpu_batch,
        op_gpu_describe,
        op_gpu_validate_device,
        op_gpu_events,
        op_gpu_event_check
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
    #[tokio::test(flavor = "current_thread")]
    #[ignore = "real GPU required"]
    #[allow(deprecated)]
    async fn native_runtime_js_event_delivery() {
        use crate::engine::opa::{EvalMode, PolicyChain};
        crate::engine::initialize_v8();
        let mut runtime = deno_core::JsRuntime::new(deno_core::RuntimeOptions {
            extensions: vec![
                create_extension(),
                crate::engine::timers::create_extension(),
            ],
            ..Default::default()
        });
        runtime
            .op_state()
            .borrow_mut()
            .put(GpuConfig::new_with_hooks(Arc::new(HookChain::from_policy(
                "gpu",
                Arc::new(PolicyChain::new(vec![], EvalMode::All)),
            ))));
        runtime
            .execute_script("setup", "globalThis.navigator={};")
            .unwrap();
        inject(&mut runtime).unwrap();
        let ready=runtime.execute_script("ready",r#"(async()=>{
            globalThis.d=await(await navigator.gpu.requestAdapter()).requestDevice();
            globalThis.observed=new Promise(resolve=>{
                d.onuncapturederror=e=>{if(!(e.error instanceof GPUValidationError))throw Error('wrong native class');resolve(e.error.message);};
            });
        })()"#).unwrap();
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            runtime.resolve_value(ready),
        )
        .await
        .unwrap()
        .unwrap();
        let session = runtime
            .op_state()
            .borrow()
            .borrow::<GpuState>()
            .session
            .clone()
            .unwrap();
        let _invalid = session
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: None,
                source: wgpu::ShaderSource::Wgsl("invalid WGSL".into()),
            });
        session.device.poll(wgpu::PollType::Wait).unwrap();
        let observed = runtime
            .execute_script(
                "observe",
                "Promise.all([Deno.core.ops.op_timer_sleep(20),observed]).then(([,message])=>{if(!message.length)throw Error('missing error');})",
            )
            .unwrap();
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            runtime.resolve_value(observed),
        )
        .await
        .unwrap()
        .unwrap();
        session.device.destroy();
        let _ = session.device.poll(wgpu::PollType::Wait);
        let lost=runtime.execute_script("lost","Promise.all([Deno.core.ops.op_timer_sleep(20),d.lost]).then(([,info])=>{if(info.reason!=='destroyed')throw Error('missing native loss');})").unwrap();
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            runtime.resolve_value(lost),
        )
        .await
        .unwrap()
        .unwrap();
    }
    #[test]
    #[ignore = "real GPU required"]
    fn native_destroy_returns_queued_and_drained_terminal_outcomes() {
        use crate::engine::opa::{EvalMode, PolicyChain};
        crate::engine::initialize_v8();
        let instance = wgpu::Instance::new(&Default::default());
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
        for drained in [false, true] {
            for overflow in [false, true] {
                let (device, queue) =
                    pollster::block_on(adapter.request_device(&Default::default())).unwrap();
                let collector = events::attach(&device);
                collector.inject_terminal_for_test(overflow);
                if drained {
                    let _ = pollster::block_on(collector.receive()).unwrap();
                }
                let mut runtime = deno_core::JsRuntime::new(deno_core::RuntimeOptions {
                    extensions: vec![create_extension()],
                    ..Default::default()
                });
                let state = runtime.op_state();
                state.borrow_mut().put(GpuConfig::new_with_hooks(Arc::new(
                    HookChain::from_policy(
                        "gpu",
                        Arc::new(PolicyChain::new(vec![], EvalMode::All)),
                    ),
                )));
                state.borrow_mut().put(GpuState {
                    requested: true,
                    submissions: 0,
                    session: Some(Arc::new(Session {
                        token: "race-test".into(),
                        device,
                        queue,
                        busy: Mutex::new(()),
                        resources: Mutex::new(ResourceRegistry::default()),
                        info: serde_json::json!({}),
                        events: collector.clone(),
                        _permit: capacity().unwrap(),
                    })),
                });
                runtime.execute_script("destroy-race",r#"
                    const outcome=JSON.parse(Deno.core.ops.op_gpu_destroy('race-test'));
                    if(outcome.overflow!==OVERFLOW)throw Error('overflow lost');
                    if(OVERFLOW){if(outcome.loss!==null)throw Error('fabricated loss');}
                    else if(outcome.loss.kind!=='unknown'||outcome.loss.message!=='earlier native loss')throw Error('native loss overwritten');
                "#.replace("OVERFLOW",if overflow {"true"}else{"false"})).unwrap();
                assert!(state.borrow().borrow::<GpuState>().session.is_none());
                assert!(session_for_token(&state.borrow(), "race-test").is_err());
                // Runtime cleanup cannot replace a terminal record already returned.
                drop(runtime);
                let repeated = collector.close();
                assert_eq!(repeated.overflow, overflow);
            }
        }
    }
    #[test]
    fn resource_identity_bounds() {
        assert!(validate_refs(&[BufferRef { id: 0, revision: 0 }], 1).is_err());
        assert!(
            validate_refs(
                &[BufferRef {
                    id: 129,
                    revision: 0
                }],
                1
            )
            .is_err()
        );
        assert!(validate_refs(&[BufferRef { id: 1, revision: 0 }; 2], 2).is_err());
        assert!(validate_refs(&[BufferRef { id: 1, revision: 0 }], 2).is_err());
    }
    #[test]
    #[ignore = "real GPU required"]
    fn resident_identity_revision_and_budget() {
        let instance = wgpu::Instance::new(&Default::default());
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            required_limits: wgpu::Limits::downlevel_defaults(),
            ..Default::default()
        }))
        .unwrap();
        let session = Session {
            token: "test-session".into(),
            device,
            queue,
            busy: Mutex::new(()),
            resources: Mutex::new(ResourceRegistry::default()),
            info: serde_json::json!({}),
            events: Arc::new(events::Collector::default()),
            _permit: capacity().unwrap(),
        };
        let job = |value: u32, revision| {
            Job {
            code: "@group(0) @binding(0) var<storage,read_write> a:array<u32>; @compute @workgroup_size(1) fn main(){a[0]+=1u;}".into(),
            entry: "main".into(), workgroups: [1;3], buffers: vec![value.to_le_bytes().to_vec()], uniforms: vec![], refs: vec![BufferRef {id:1,revision}]
        }
        };
        let output = |value, revision| {
            let raw = run(&session, job(value, revision)).unwrap();
            let bytes: Vec<Vec<u8>> = serde_json::from_str(&raw).unwrap();
            u32::from_le_bytes(bytes[0].clone().try_into().unwrap())
        };
        assert_eq!(output(0, 0), 1);
        // Same ID/revision must ignore stale host bytes and use GPU output.
        assert_eq!(output(99, 0), 2);
        assert_eq!(output(10, 1), 11);
        assert!(run(&session, job(0, 0)).is_err());
        assert!(
            resident_buffers(
                &session,
                &[vec![0; 8]],
                &[140],
                &[BufferRef { id: 1, revision: 1 }]
            )
            .is_err()
        );
        assert!(
            resident_buffers(
                &session,
                &[vec![0; 4]],
                &[76],
                &[BufferRef { id: 1, revision: 1 }]
            )
            .is_err()
        );
        resident_buffers(
            &session,
            &[vec![0; MAX_BYTES - 4]],
            &[140],
            &[BufferRef { id: 2, revision: 0 }],
        )
        .unwrap();
        assert!(
            resident_buffers(
                &session,
                &[vec![0; 4]],
                &[140],
                &[BufferRef { id: 3, revision: 0 }]
            )
            .is_err()
        );
        assert_eq!(session.resources.lock().unwrap().bytes, MAX_BYTES);
        // Adding VERTEX is an identity mutation even when the uniform bit agrees.
        assert!(
            resident_buffers(
                &session,
                &[vec![0; 4]],
                &[172],
                &[BufferRef { id: 1, revision: 1 }]
            )
            .is_err()
        );
    }
    #[test]
    fn declared_usage_validation() {
        assert!(validate_buffer_usages(&[vec![0; 48]], &[108]).is_ok());
        for usage in [0, 2, 129, 192, 256] {
            assert!(validate_buffer_usages(&[vec![0; 4]], &[usage]).is_err());
        }
        assert!(validate_buffer_usages(&[vec![0; 4]], &[]).is_err());
        assert!(validate_buffer_usages(&[vec![0; 65540]], &[108]).is_err());
    }
    #[test]
    fn fixed_native_profile_requests() {
        for (name, value) in [
            ("low-power", wgpu::PowerPreference::LowPower),
            ("high-performance", wgpu::PowerPreference::HighPerformance),
        ] {
            let a: AdapterRequest = serde_json::from_value(
                serde_json::json!({"powerPreference":name,"forceFallbackAdapter":true}),
            )
            .unwrap();
            assert_eq!(a.preference().unwrap(), value);
            assert!(a.force_fallback_adapter);
        }
        let bad: AdapterRequest =
            serde_json::from_value(serde_json::json!({"powerPreference":"fastest"})).unwrap();
        assert!(bad.preference().is_err());
        assert!(
            serde_json::from_value::<AdapterRequest>(
                serde_json::json!({"forceFallbackAdapter":"false"})
            )
            .is_err()
        );
        let limits = effective_limits(&wgpu::Limits::downlevel_defaults());
        let exact: DeviceRequest =
            serde_json::from_value(serde_json::json!({"requiredLimits":limits})).unwrap();
        assert!(validate_device_request(&exact, &limits).is_ok());
        assert_eq!(limits["maxBindGroups"], 1);
        assert_eq!(limits["maxSamplersPerShaderStage"], 0);
        assert_eq!(limits["maxBufferSize"], MAX_BYTES);
        assert_eq!(limits["maxTextureDimension2D"], 1024);
        let mut wrong = exact;
        wrong.required_limits.insert("maxBufferSize".into(), 4);
        assert!(validate_device_request(&wrong, &limits).is_err());
        assert!(
            validate_device_request(
                &DeviceRequest {
                    required_features: vec!["shader-f16".into()],
                    ..Default::default()
                },
                &limits
            )
            .is_err()
        );
        let unknown: DeviceRequest =
            serde_json::from_value(serde_json::json!({"requiredLimits":{"unknown":0}})).unwrap();
        assert!(validate_device_request(&unknown, &limits).is_err());
    }
    #[test]
    fn native_bounds() {
        assert!(serde_json::from_str::<Job>(r#"{"code":"","entry":"main","workgroups":[1,1,1],"buffers":[[],[],[],[],[],[],[],[],[]]}"#).is_err());
        let mut j = Job {
            code: String::new(),
            entry: "main".into(),
            workgroups: [1; 3],
            buffers: vec![vec![0; 4]],
            uniforms: vec![],
            refs: vec![],
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
