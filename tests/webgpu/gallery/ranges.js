// Pooled allocations: native binding ranges and copies must preserve sentinels.
const pool=device.createBuffer({size:1024,usage:140}),uniform=device.createBuffer({size:512,usage:72});
const destination=device.createBuffer({size:128,usage:140}),out=device.createBuffer({size:64,usage:9}),poolOut=device.createBuffer({size:1024,usage:9});
const initial=new Float32Array(256).fill(-123.5);initial.set([-4,-3,-2,-1,0,1,2,3],64);
device.queue.writeBuffer(pool,0,initial);device.queue.writeBuffer(uniform,256,new Float32Array([2,3,0,0]));device.queue.writeBuffer(destination,0,new Float32Array(32).fill(99));
const p=device.createComputePipeline({layout:'auto',compute:{module:device.createShaderModule({code:'@group(0) @binding(0) var<storage,read_write> values:array<f32>; @group(0) @binding(1) var<uniform> params:vec4f; @compute @workgroup_size(64) fn main(@builtin(global_invocation_id) id:vec3u){if(id.x<arrayLength(&values)){values[id.x]=values[id.x]*params.x+params.y;}}'}),entryPoint:'main'}});
const resource={buffer:pool,offset:256,size:32};
const g=device.createBindGroup({layout:p.getBindGroupLayout(0),entries:[{binding:0,resource},{binding:1,resource:{buffer:uniform,offset:256,size:16}}]});
resource.offset=0;resource.size=1024; // Binding descriptors are captured at creation.
const e=device.createCommandEncoder(),pass=e.beginComputePass();pass.setPipeline(p);pass.setBindGroup(0,g);pass.dispatchWorkgroups(1);pass.end();
e.copyBufferToBuffer(pool,256,destination,4,32);e.copyBufferToBuffer(pool,1020,destination,60,4);e.copyBufferToBuffer(destination,0,out,0,64);e.copyBufferToBuffer(pool,0,poolOut,0,1024);device.queue.submit([e.finish()]);
await out.mapAsync(1);const actual=new Float32Array(out.getMappedRange());for(let i=0;i<16;i++){const expected=i>=1&&i<=8?2*(i-5)+3:i===15?-123.5:99;assert(actual[i]===expected,`partial-copy sentinel ${i}: ${actual[i]}`);}out.unmap();
await poolOut.mapAsync(1);const all=new Float32Array(poolOut.getMappedRange());for(let i=0;i<256;i++){const expected=i>=64&&i<72?2*initial[i]+3:-123.5;assert(all[i]===expected,`binding-range sentinel ${i}`);}poolOut.unmap();device.destroy();
