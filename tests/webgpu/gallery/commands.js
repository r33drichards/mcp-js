// Independent CPU oracle for ordered upload/copy/dispatch/pass dependencies.
const n=257, bytes=n*4, initial=Float32Array.from({length:n},(_,i)=>(i-128)/8);
const input=device.createBuffer({size:bytes,usage:12});
const work=device.createBuffer({size:bytes,usage:140});
const middle=device.createBuffer({size:bytes,usage:140});
const out=device.createBuffer({size:bytes,usage:9});
device.queue.writeBuffer(input,0,initial);
function pipeline(expression){return device.createComputePipeline({layout:'auto',compute:{module:device.createShaderModule({code:`@group(0) @binding(0) var<storage,read_write> a:array<f32>; @compute @workgroup_size(64) fn main(@builtin(global_invocation_id) id:vec3u){if(id.x<arrayLength(&a)){a[id.x]=${expression};}}`}),entryPoint:'main'}});}
const add=pipeline('a[id.x]+1.0'),mul=pipeline('a[id.x]*2.0');
function group(p,b){return device.createBindGroup({layout:p.getBindGroupLayout(0),entries:[{binding:0,resource:{buffer:b}}]});}
const aw=group(add,work),mw=group(mul,work),am=group(add,middle);
const e=device.createCommandEncoder();e.copyBufferToBuffer(input,0,work,0,bytes);
const p=e.beginComputePass();p.setPipeline(add);p.setBindGroup(0,aw);p.dispatchWorkgroups(5);p.setPipeline(mul);p.setBindGroup(0,mw);p.dispatchWorkgroups(5);p.end();
e.copyBufferToBuffer(work,0,middle,0,bytes);
const p2=e.beginComputePass();p2.setPipeline(add);p2.setBindGroup(0,am);p2.dispatchWorkgroups(5);p2.end();
e.copyBufferToBuffer(middle,0,out,0,bytes);device.queue.submit([e.finish()]);await out.mapAsync(1);
const actual=new Float32Array(out.getMappedRange());for(let i=0;i<n;i++)assert(Math.abs(actual[i]-(2*initial[i]+3))<=1e-6,`ordered command mismatch ${i}`);out.unmap();
// Copy-only commands must read the resident result, then observe a later upload.
const copy=device.createCommandEncoder();copy.copyBufferToBuffer(middle,0,out,0,bytes);device.queue.submit([copy.finish()]);await out.mapAsync(1);assert(new Float32Array(out.getMappedRange())[0]===-29,'resident copy-only result');out.unmap();
device.queue.writeBuffer(input,0,new Float32Array(n).fill(7));const copy2=device.createCommandEncoder();copy2.copyBufferToBuffer(input,0,middle,0,bytes);copy2.copyBufferToBuffer(middle,0,out,0,bytes);device.queue.submit([copy2.finish()]);await out.mapAsync(1);assert(new Float32Array(out.getMappedRange()).every(x=>x===7),'ordered copy/upload result');out.unmap();device.destroy();
