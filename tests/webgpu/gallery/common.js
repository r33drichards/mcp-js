// Test harness: CPU uploads/readback replace the gallery's browser/render loop.
function assert(ok, message) { if (!ok) throw Error(message); }
function rng(seed) { return () => { seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0; return seed / 4294967296; }; }
const adapter = await navigator.gpu.requestAdapter();
assert(adapter, 'Real GPU required; no adapter');
const device = await adapter.requestDevice();
function program(code, entry, sizes, outputs, uniforms = []) {
  const storage = sizes.map((size,i) => device.createBuffer({size, usage: (uniforms.includes(i) ? GPUBufferUsage.UNIFORM : GPUBufferUsage.STORAGE) | GPUBufferUsage.COPY_SRC | GPUBufferUsage.COPY_DST}));
  const readback = outputs.map(i => device.createBuffer({size:sizes[i], usage: GPUBufferUsage.MAP_READ | GPUBufferUsage.COPY_DST}));
  const pipeline = device.createComputePipeline({layout:'auto', compute:{module:device.createShaderModule({code}), entryPoint:entry}});
  const group = device.createBindGroup({layout:pipeline.getBindGroupLayout(0), entries:storage.map((buffer,binding) => ({binding, resource:{buffer}}))});
  return async (inputs, workgroups) => {
    inputs.forEach((input,i) => device.queue.writeBuffer(storage[i],0,input));
    const encoder = device.createCommandEncoder(); const pass = encoder.beginComputePass();
    pass.setPipeline(pipeline); pass.setBindGroup(0,group); pass.dispatchWorkgroups(...workgroups); pass.end();
    outputs.forEach((i,j) => encoder.copyBufferToBuffer(storage[i],0,readback[j],0,sizes[i]));
    device.queue.submit([encoder.finish()]);
    const results = [];
    for (const b of readback) { await b.mapAsync(GPUMapMode.READ); results.push(b.getMappedRange().slice(0)); b.unmap(); }
    return results;
  };
}
