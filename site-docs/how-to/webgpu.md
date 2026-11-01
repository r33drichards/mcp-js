# WebGPU compute and offscreen rendering (experimental subset)

mcp-js can run WGSL compute shaders through Rust [wgpu](https://github.com/gfx-rs/wgpu).
GPU access is off by default. Configure a `gpu` policy section to expose
`navigator.gpu`, `GPUBufferUsage`, `GPUTextureUsage`, and `GPUMapMode`.

This is an experimental subset, not a conformant WebGPU implementation. It supports
compute and a limited offscreen rendering path. It has no browser canvas, texture
sampling, indexed draws, timestamps, indirect dispatch, optional device features,
or general error scopes/EventTarget listeners. A bounded `onuncapturederror` handler
and `device.lost` bridge are available as described below. See `docs/webgpu-rendering-plan.md` in the source checkout for tested samples
and remaining work.

## Enable access

Add this section to the file passed to `--policies-json`:

```json
{"gpu":{"policies":[{"url":"file:///absolute/path/gpu.rego"}]}}
```

Example `gpu.rego`:

```rego
package mcp.gpu

default allow = false
allow if { input.operation == "requestAdapter" }
allow if {
    input.operation == "dispatch"
    input.bytes <= 1048576
}
allow if {
    input.operation == "copyBuffer"
    input.bytes <= 1048576
}
```

The adapter gate runs before native GPU initialization. Each dispatch passes
`{"operation":"dispatch","bytes":N,"workgroups":[x,y,z]}` to the same hook chain.
Each native buffer copy passes `{"operation":"copyBuffer","bytes":N}`. All
operations in a batch must be allowed before native work begins. Policy `bytes`
counts full referenced allocations; for a copy it is the sum of the complete
source and destination allocations, even when only a small range is copied.
`sourceBytes` and `destinationBytes` report these allocations separately;
`bindingBytes` and `copiedBytes` additionally report selected range sizes, with
`sourceOffset`/`destinationOffset` for copies. Partial ranges do not reduce the
allocation-based policy check. This is stricter than the earlier source-only
copy accounting: an equal-sized copy now reports twice one buffer's size. Policies that
previously allowed only dispatch must explicitly allow copies when used.
These are gate-only hooks: mutation and post hooks are unsupported. An explicit
`{"gpu":{"policies":[]}}` permits access within the built-in limits. Omit `gpu`
to disable access completely. The Rust embedding API uses `Engine::with_gpu_config`.

## Upload, dispatch, read back

```js
const adapter = await navigator.gpu.requestAdapter();
if (!adapter) throw new Error("No GPU adapter available");
const device = await adapter.requestDevice();
const input = new Uint32Array([1, 2, 3, 4]);
const storage = device.createBuffer({
  size: input.byteLength,
  usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_SRC | GPUBufferUsage.COPY_DST
});
const output = device.createBuffer({
  size: input.byteLength,
  usage: GPUBufferUsage.MAP_READ | GPUBufferUsage.COPY_DST
});
device.queue.writeBuffer(storage, 0, input);
const module = device.createShaderModule({code: `
  @group(0) @binding(0) var<storage, read_write> values: array<u32>;
  @compute @workgroup_size(1)
  fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    values[id.x] *= 2u;
  }
`});
const pipeline = device.createComputePipeline({
  layout: "auto", compute: {module, entryPoint: "main"}
});
const group = device.createBindGroup({
  layout: pipeline.getBindGroupLayout(0),
  entries: [{binding: 0, resource: {buffer: storage}}]
});
const encoder = device.createCommandEncoder();
const pass = encoder.beginComputePass();
pass.setPipeline(pipeline);
pass.setBindGroup(0, group);
pass.dispatchWorkgroups(4);
pass.end();
encoder.copyBufferToBuffer(storage, 0, output, 0, input.byteLength);
device.queue.submit([encoder.finish()]);
await output.mapAsync(GPUMapMode.READ);
const result = new Uint32Array(output.getMappedRange()).slice();
output.unmap(); // detaches the mapped range
console.log(Array.from(result)); // [2, 4, 6, 8]
device.destroy();
```

## Supported contract and limits

- One adapter request and one device per execution; two active native sessions
  per server process. Missing adapters return `null`; device creation failures reject.
  Adapter `powerPreference` and `forceFallbackAdapter` are validated and forwarded
  to wgpu; forced fallback availability depends on the backend. Adapter info comes
  from native wgpu (unknown architecture is an empty string). Adapter/device
  limits describe the effective implemented subset, clamped to native capacity and
  per-object ceilings. Unsupported binding categories report zero. Cumulative
  allocation/work quotas still apply; limits are not a conformance claim. Features
  are empty because the native
  device enables no optional features. `requestDevice` accepts only empty features
  and supplied limit values exactly matching that effective profile; lower/higher
  requests and unknown limits are rejected, not silently ignored. Arbitrary
  device negotiation remains unsupported.
- At most 4 MiB of cumulative JavaScript buffer allocation per device, including
  readback buffers. Destroying a buffer does not refund this budget. Buffers must
  have positive sizes divisible by four. Supported usage flags are `STORAGE`, `UNIFORM`, `VERTEX`,
  `COPY_SRC`, `COPY_DST`, and `MAP_READ`; `MAP_READ` may combine only with `COPY_DST`.
- One bind group at index zero, with 1–8 distinct storage or uniform buffers bound consecutively
  from zero. Static binding ranges and automatic pipeline layouts are supported. Binding offsets
  must be multiples of 256; sizes must be positive multiples of four within the
  allocation. Aliased bindings and dynamic offsets remain unsupported.
- Multiple compute passes/dispatches and up to eight ordered buffer copies
  per command buffer, including copy-only buffers. Copies require no active pass, distinct buffers, and positive four-byte-aligned
  sizes/offsets within both allocations; source and destination sizes may differ.
  One command buffer per submit, with a FIFO and one active native operation. The execution has 32
  work units: each compute dispatch, render, writeBuffer upload or copy-only
  submission consumes one. Units are reserved at admission and never refunded. Batching cannot multiply this allowance. Workgroup counts must be
  positive, with product at most 65,536 per dispatch; wgpu enforces device limits.
- A compute/copy batch references at most 16 distinct buffers (still at most eight
  bindings per dispatch), with at most 4 MiB total bytes and 64 KiB aggregate WGSL
  across its commands. The native encoder executes ordered copies and dispatches
  without intermediate host readbacks. Individual JS dispatches become native
  compute passes to order their storage dependencies.
- Uniform buffers are limited to 64 KiB each and cannot also have STORAGE usage.
- At most 128 created buffer/shader/pipeline/bind-group/encoder/texture/view objects per device;
  WGSL source is limited to 64 KiB and entry points to 256 bytes on the native boundary.
- `queue.writeBuffer` snapshots only the selected bytes at call time and queues
  the upload in FIFO order. Snapshot bytes count against the existing cumulative
  4 MiB resource allowance, with no refund, and each accepted upload costs one
  work unit. Source mutation or detachment afterward cannot change the upload.
  ArrayBuffer and typed-array/DataView sources are supported; shared buffers are
  outside this subset. At execution, the upload patches the host mirror and
  advances its revision. Native upload remains deferred until a GPU operation
  passes its existing full-allocation policy checks. Native buffers persist within an
  execution, keyed by session-local identity and upload revision. Unchanged
  revisions reuse GPU contents, including binding a compute output as a vertex
  buffer on a later submission. Explicit host writes advance the revision;
  native buffer copies retain it. All batch buffers are eagerly read back after
  the batch, and serialized jobs still include CPU bytes; reuse skips their upload.
  The separate render path still copies its readback into the JS destination and
  advances that destination's revision.
- The resident buffer registry preserves each buffer's complete declared usage
  mask across compute, copy and render paths. `UNIFORM | VERTEX` is supported;
  an identity cannot change its declared usage after registration. Native buffers
  include internal copy capabilities for staging; public operations still require
  their declared usage flags. MAP_READ uses separate staging buffers.
- The resident buffer registry independently enforces 4 MiB and at most 128 IDs.
  These are logical resource limits, not an exact VRAM ceiling: staging, pipelines
  and driver allocations are additional. Buffer destruction invalidates its JS
  wrapper immediately; already-admitted jobs retain its charged backing store until their last use. Resident storage is retained until device/session cleanup,
  and the cumulative budget is not refunded. Native handles never enter snapshots.
- Mapping supports full-buffer `MAP_READ` only, one mapped range at a time.
  `mappedAtCreation` supports one full-buffer initialization range with zeroing and detach-on-unmap/destruction. `MAP_WRITE` is restricted to `COPY_SRC`; subsequent write mapping, partial mapping, and dynamic offsets are unsupported.
- Native shader/pipeline validation is deferred until submission. Errors reject
  `queue.onSubmittedWorkDone()` and dependent `mapAsync()` calls, rather than using
  WebGPU error scopes or `uncapturederror`. A failed submitted job cancels the queued suffix and terminates the session conservatively. Always await one of these promises.
- Active plus waiting jobs retain at most 64 KiB of UTF-8 serialized metadata,
  128 buffer references and 32 job records, within the cumulative work allowance.
  Waiting GPU jobs retain existing charged stores; upload entries additionally retain their charged selected-byte snapshots.
  `onSubmittedWorkDone()` and `mapAsync()` capture the already-admitted prefix;
  later submissions do not extend that fence. Pending maps block new uses, while
  earlier accepted uses remain valid.

Descriptors reject unsupported keys. The API is not yet covered by the WebGPU
conformance suite; code that relies on broader browser semantics needs adaptation.

## Offscreen rendering milestone

`createTexture`, `createView`, `createRenderPipeline`, `beginRenderPass`,
`setVertexBuffer`, `draw`, and `copyTextureToBuffer` implement one restricted path:

- One `rgba8unorm` 2D target, at most 1024×1024, with
  `RENDER_ATTACHMENT | COPY_SRC` usage. Texture bytes count against the same
  cumulative 4 MiB device budget as buffers; the readback buffer counts too.
- One clear/store render pass and one triangle-list draw per command buffer,
  followed by a whole-target readback. A fresh native target is created each
  submission. Loading or sampling a prior target is unsupported.
- At most four vertex-buffer slots, eight attributes per slot, 64 KiB total
  vertex data, and 65,536 vertex×instance invocations. Formats are float32x2/3/4;
  both vertex and instance step modes work. First vertex/instance must be zero.
- Whole-buffer vertex bindings only. No render bind groups, index buffers,
  depth/stencil, multisampling, blending, alternate formats, or samplers yet.
- Readback offset must be zero; bytesPerRow must be a multiple of 256 and at
  least width×4. The destination must contain exactly bytesPerRow×height bytes.
- Compute and render passes use separate ordered FIFO submissions. A shared STORAGE | VERTEX buffer stays resident across
  them; eager readback still occurs, but the render input is not re-uploaded.

Each native rendering submission independently checks the existing GPU policy
with `operation: "render"`, `width`, `height`, `bytes` (unpadded color target),
and `draw: [vertexCount, instanceCount, 0, 0]`. Denial occurs before shader
compilation or rendering. Render and compute share the 32-work-unit budget,
two-session process cap, execution tokens, and no-hard-preemption limitation.

## Cleanup, snapshots, and execution limits

GPU handles live in Rust OpState. A random execution token identifies the session;
no native pointer is serialized into the V8 heap. Native sessions drop when an
execution ends. `device.destroy()` releases its session; a submitted worker retains
its handles until it finishes. Pending work therefore cannot be freed underneath
the driver. Restored devices, adapters, and buffers reject operations as stale,
even after a fresh adapter is created. Request a new adapter each turn. CPU-side
buffer data may remain in the snapshot, subject to the normal heap limit.

Native validation repeats the byte, shader, binding, dispatch, and submission
bounds independently of JavaScript checks. GPU storage and readback allocations
are each bounded by 4 MiB per submission; driver/compiler overhead is additional.
The native readback loop stops after 30 seconds and destroys the device.

These limits do **not** guarantee a GPU execution-time bound. Arbitrary WGSL can
contain long or infinite loops; compilation, driver calls, and device destruction
can also block. V8 cancellation cannot preempt an already submitted kernel. Enable
this capability only for workloads trusted to use the GPU; process isolation and
OS/driver watchdogs are needed for stronger availability guarantees. Existing OS
sandbox settings are unchanged; a restrictive host sandbox may deny GPU access.

## Binding choice and licensing

We evaluated [deno_webgpu](https://github.com/gfx-rs/wgpu/tree/trunk/deno_webgpu).
Its MIT-licensed bindings provide a much broader API, but depend on Deno's
`deno_webidl` and `deno_web` extensions and native GC objects. mcp-js instead uses
custom web shims, a pinned deno_core fork, and execution snapshots. Reusing those
bindings would require a larger compatibility and handle-lifecycle integration.

This implementation uses original, limited bindings over wgpu 26 (MIT OR Apache-2.0)
and pollster (MIT OR Apache-2.0). These licenses permit use in this AGPL-3.0 project;
redistributions must retain dependency notices. No Deno binding code was copied.
Reconsider deno_webgpu if full WebGPU compatibility becomes a requirement.

## Tests

```sh
node --test tests/webgpu/shim.test.cjs
cargo test -p server --lib engine::webgpu::tests
cargo test -p server --test webgpu
# Requires a real GPU; missing adapters fail these explicit tests:
cargo test -p server --test webgpu -- --ignored --test-threads=1
```

Use the repository's Nix Rust environment. The JavaScript tests mock the native
boundary; only the explicitly enabled Rust GPU tests verify actual WGSL execution.


## Native error and loss delivery

`device.onuncapturederror` accepts one function or null. Actual uncaptured wgpu
errors arrive with `event.error` classified as `GPUValidationError`,
`GPUOutOfMemoryError`, or `GPUInternalError`. The small event facade supports
`preventDefault()` and `defaultPrevented`; it is not a full EventTarget API.
`device.lost` resolves on native loss or explicit `device.destroy()`.
Existing internal native error scopes still reject invalid submissions and do not
also emit duplicate uncaptured events. JavaScript push/pop error scopes remain
unsupported.

The receiver is unreferenced: merely holding or awaiting `device.lost` does not
keep an otherwise idle execution alive. Delivery occurs while the runtime is
active; teardown closes/wakes the receiver and destroys native resources. Callback
captures are weak. Native token and terminal state are checked at delivery, even
for events already drained from the queue, and before each JS handler invocation.

At most 16 error messages of 1024 UTF-8 bytes are retained, plus a separate first
native-loss record. This bounds retained text, not native error construction or
formatting CPU time. Queue overflow terminates the session and **rejects** the
loss promise with an implementation error; it never fabricates native device
loss. Destroy atomically preserves earlier native loss or overflow, even after
the receiver has drained those records. A throwing handler or receiver failure
also terminates the bridge/session and rejects that promise unless an earlier
authoritative native terminal outcome already exists. This fail-closed behavior is an explicit experimental
subset limitation, not standard browser exception reporting. No GPU hard
preemption is promised for work already submitted.


## Verified ONNX smoke scope

Pinned ONNX Runtime Web 1.23.2 all/JSEP assets execute a single float32 Add node
with inputs [1,2,3,4] and [10,20,30,40], returning [11,22,33,44]. The opt-in test
selects only the WebGPU provider, disables CPU EP fallback, checks the runtime's
configuration report, and requires a native-dispatch-denied control to fail.
This verifies this tiny model, not general ONNX model/operator compatibility.
Assets remain external and hash-verified; the existing heap, GPU policy and
resource budgets apply. Larger workloads can exceed the cumulative staging or
work limits. The bitonic numerical gallery test now checks bounded phase windows
using CPU-reference prefix/suffix state; the original full-sort host harness
exceeds 32 units with upload accounting and is not supported unchanged.
