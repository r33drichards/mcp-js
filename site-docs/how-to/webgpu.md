# WebGPU compute (experimental subset)

mcp-js can run WGSL compute shaders through Rust [wgpu](https://github.com/gfx-rs/wgpu).
GPU access is off by default. Configure a `gpu` policy section to expose
`navigator.gpu`, `GPUBufferUsage`, and `GPUMapMode`.

This is a compute subset, not a conformant WebGPU implementation. It uses familiar
WebGPU calls for upload, dispatch, and readback. It has no rendering, textures,
canvas, timestamps, indirect dispatch, optional device features, or error events.

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
```

The adapter gate runs before native GPU initialization. Each dispatch passes
`{"operation":"dispatch","bytes":N,"workgroups":[x,y,z]}` to the same hook chain.
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
  Adapter/device options and optional features are unsupported.
- At most 4 MiB of cumulative JavaScript buffer allocation per device, including
  readback buffers. Destroying a buffer does not refund this budget. Buffers must
  have positive sizes divisible by four. Supported usage flags are `STORAGE`,
  `COPY_SRC`, `COPY_DST`, and `MAP_READ`; `MAP_READ` may combine only with `COPY_DST`.
- One bind group at index zero, with 1–8 distinct storage buffers bound consecutively
  from zero. Only whole-buffer bindings and automatic pipeline layouts are supported.
- One compute pass, one dispatch, and up to eight subsequent whole-buffer copies
  per command buffer. One command buffer per submit, one submit in flight, and
  at most 32 submits per execution. Workgroup counts must be positive, with product
  at most 65,536. wgpu also enforces device limits, including workgroup size.
- At most 128 created buffer/shader/pipeline/bind-group/encoder objects per device;
  WGSL source is limited to 64 KiB and entry points to 256 bytes on the native boundary.
- Buffer upload uses `queue.writeBuffer`. Native buffers are staged from CPU data
  at submission and all storage bindings are read back after dispatch. This favors
  a small, auditable implementation over throughput and GPU-resident data reuse.
- Mapping supports full-buffer `MAP_READ` only, one mapped range at a time.
  `mappedAtCreation`, write mapping, offsets in bindings, and dynamic offsets are unsupported.
- Native shader/pipeline validation is deferred until submission. Errors reject
  `queue.onSubmittedWorkDone()` and dependent `mapAsync()` calls, rather than using
  WebGPU error scopes or `uncapturederror`. Always await one of these promises.

Descriptors reject unsupported keys. The API is not yet covered by the WebGPU
conformance suite; code that relies on broader browser semantics needs adaptation.

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
