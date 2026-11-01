const values=new Float32Array([-.5,-.5,0,1, .5,-.5,0,1, 0,.5,0,1]);
const shared=device.createBuffer({size:48,usage:GPUBufferUsage.UNIFORM|GPUBufferUsage.VERTEX|GPUBufferUsage.COPY_SRC|GPUBufferUsage.COPY_DST});
if(CASE.order.startsWith('copy')){
 const source=device.createBuffer({size:48,usage:12});device.queue.writeBuffer(source,0,values);
 const e=device.createCommandEncoder();e.copyBufferToBuffer(source,0,shared,0,48);device.queue.submit([e.finish()]);await device.queue.onSubmittedWorkDone();
}else device.queue.writeBuffer(shared,0,values);
async function compute(){
 const output=device.createBuffer({size:16,usage:140}),read=device.createBuffer({size:16,usage:9});
 const p=device.createComputePipeline({layout:'auto',compute:{module:device.createShaderModule({code:'@group(0) @binding(0) var<uniform> points:array<vec4f,3>; @group(0) @binding(1) var<storage,read_write> result:array<f32>; @compute @workgroup_size(1) fn main(){result[0]=points[0].x;}'}),entryPoint:'main'}});
 const g=device.createBindGroup({layout:p.getBindGroupLayout(0),entries:[{binding:0,resource:{buffer:shared}},{binding:1,resource:{buffer:output}}]});
 const e=device.createCommandEncoder(),pass=e.beginComputePass();pass.setPipeline(p);pass.setBindGroup(0,g);pass.dispatchWorkgroups(1);pass.end();e.copyBufferToBuffer(output,0,read,0,16);device.queue.submit([e.finish()]);await read.mapAsync(1);assert(new Float32Array(read.getMappedRange())[0]===-.5,'uniform union read mismatch');read.unmap();
}
async function render(){
 const target=device.createTexture({size:[64,64],format:'rgba8unorm',usage:17}),read=device.createBuffer({size:16384,usage:9});
 const m=device.createShaderModule({code:'@vertex fn vs(@location(0) p:vec4f)->@builtin(position) vec4f{return p;} @fragment fn fs()->@location(0) vec4f{return vec4f(1,0,0,1);}'});
 const p=device.createRenderPipeline({layout:'auto',vertex:{module:m,entryPoint:'vs',buffers:[{arrayStride:16,attributes:[{shaderLocation:0,offset:0,format:'float32x4'}]}]},fragment:{module:m,entryPoint:'fs',targets:[{format:'rgba8unorm'}]}});
 const e=device.createCommandEncoder(),pass=e.beginRenderPass({colorAttachments:[{view:target.createView(),clearValue:[0,0,0,1],loadOp:'clear',storeOp:'store'}]});pass.setPipeline(p);pass.setVertexBuffer(0,shared);pass.draw(3);pass.end();e.copyTextureToBuffer({texture:target},{buffer:read,bytesPerRow:256},[64,64]);device.queue.submit([e.finish()]);await read.mapAsync(1);const bytes=new Uint8Array(read.getMappedRange());const center=(32*64+32)*4;assert(bytes[center]===255&&bytes[center+1]===0&&bytes[center+3]===255,'union vertex render center');assert(bytes[0]===0&&bytes[3]===255,'union vertex render background');read.unmap();
}
if(CASE.order.endsWith('render-compute')){await render();await compute();}else{await compute();await render();}
device.destroy();
