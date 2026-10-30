const adapter = await navigator.gpu.requestAdapter();
if (!adapter) throw new Error('GPU adapter unavailable');
const device = await adapter.requestDevice();
const storage = device.createBuffer({size: 16, usage: GPUBufferUsage.STORAGE | GPUBufferUsage.COPY_SRC | GPUBufferUsage.COPY_DST});
const readback = device.createBuffer({size: 16, usage: GPUBufferUsage.MAP_READ | GPUBufferUsage.COPY_DST});
device.queue.writeBuffer(storage, 0, new Uint32Array([1, 2, 3, 4]));
const module = device.createShaderModule({code: `
@group(0) @binding(0) var<storage, read_write> values: array<u32>;
@compute @workgroup_size(1) fn main(@builtin(global_invocation_id) id: vec3<u32>) {
  values[id.x] = values[id.x] * 2u;
}`});
const pipeline = device.createComputePipeline({layout: 'auto', compute: {module, entryPoint: 'main'}});
const group = device.createBindGroup({layout: pipeline.getBindGroupLayout(0), entries: [{binding: 0, resource: {buffer: storage}}]});
const encoder = device.createCommandEncoder();
const pass = encoder.beginComputePass();
pass.setPipeline(pipeline); pass.setBindGroup(0, group); pass.dispatchWorkgroups(4); pass.end();
encoder.copyBufferToBuffer(storage, 0, readback, 0, 16);
device.queue.submit([encoder.finish()]);
await readback.mapAsync(GPUMapMode.READ);
const range = readback.getMappedRange();
const actual = Array.from(new Uint32Array(range));
if (JSON.stringify(actual) !== '[2,4,6,8]') throw new Error('wrong GPU result: ' + actual);
readback.unmap();
if (range.byteLength !== 0) throw new Error('mapped range was not detached');
// Keep a device/buffer in the snapshot to verify stale-handle rejection.
globalThis.savedGpuDevice = device;
globalThis.savedGpuBuffer = storage;
