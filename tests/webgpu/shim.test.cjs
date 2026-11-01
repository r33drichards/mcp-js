// Fast state-machine tests with a fake native boundary; real GPU coverage is in server/tests/webgpu.rs.
const { test } = require('node:test');
const assert = require('node:assert/strict');
const vm = require('node:vm');
const fs = require('node:fs');
const source = fs.readFileSync('server/src/engine/web_compat/webgpu.js', 'utf8');
function runtime() {
  let valid = true, calls = 0, deliver, outcome={loss:null,overflow:false};
  const jobs = [], requests = [];
  const c = vm.createContext({ navigator: {}, structuredClone, Deno: { core: { unrefOpPromise:()=>{}, ops: {
    op_gpu_events:()=>new Promise(resolve=>{deliver=resolve;}),
    op_gpu_event_check:()=>{if(!valid)throw Error("stale handle");},
    op_gpu_request: async json => {requests.push(JSON.parse(json));return 'token';},
    op_gpu_describe: () => JSON.stringify({limits:{maxBufferSize:4194304},features:[],info:{vendor:'test',device:'test',architecture:'',description:'mock'}}),
    op_gpu_validate_device: (_,json) => {const r=JSON.parse(json);if(r.requiredFeatures.length||Object.entries(r.requiredLimits).some(([k,v])=>k!=='maxBufferSize'||v!==4194304))throw Error('unsupported requested profile');},
    op_gpu_check: () => { if (!valid) throw Error('stale handle'); },
    op_gpu_destroy: () => { if(!valid)throw Error("stale handle");valid = false;return JSON.stringify(outcome); },
    op_gpu_batch: async (_, json) => {
      calls++;
      const job = JSON.parse(json); jobs.push(job);
      const data=job.buffers.map(b=>Uint8Array.from(b));
      for(const c of job.commands){
        if(c.kind==='copy')data[c.destination].set(data[c.source].subarray(c.source_offset,c.source_offset+c.size),c.destination_offset);
        else {const words=new Uint32Array(data[c.bindings[0].buffer].buffer,c.bindings[0].offset,c.bindings[0].size/4);for(let i=0;i<words.length;i++)words[i]*=2;}
      }
      return JSON.stringify(data.map(b=>Array.from(b)));
    }
  } } } });
  vm.runInContext(source, c);
  return { run: code => vm.runInContext('(async () => {' + code + '})()', c), stale: () => { valid = false; }, calls: () => calls, queue: value=>{if(!outcome.loss&&!outcome.overflow){outcome={loss:value.loss??null,overflow:!!value.overflow};}}, emit: value=>{if(!outcome.loss&&!outcome.overflow){outcome={loss:value.loss??null,overflow:!!value.overflow};}deliver(JSON.stringify(value));}, jobs, requests };
}
test('upload, command encoding, readback and detach', async () => {
  const r = runtime();
  await r.run(fs.readFileSync('tests/webgpu/compute.js', 'utf8'));
  assert.equal(r.calls(), 1);
});
test('bounds, unsupported descriptors, cross-device forgery and lifecycle', async () => {
  const r = runtime();
  await r.run(`
    const a = await navigator.gpu.requestAdapter(); const d = await a.requestDevice();
    const bad = f => { let failed = false; try { f(); } catch { failed = true; } if (!failed) throw Error('accepted invalid call'); };
    bad(() => d.createBuffer({size: 4194308,usage:128}));
    bad(() => d.createBuffer({size: 3,usage:128}));
    bad(() => d.createBuffer({size: 4,usage:129}));
    bad(() => d.createBuffer({size: 4,usage:128,mappedAtCreation:true}));
    bad(() => d.createShaderModule({code:'', compilationHints:[]}));
    bad(() => d.queue.writeBuffer({},0,new Uint32Array([1])));
    const b = d.createBuffer({size:4,usage:136});
    bad(() => d.queue.writeBuffer(b,0,new Uint32Array([1,2])));
    b.destroy(); bad(() => d.queue.writeBuffer(b,0,new Uint32Array([1])));
    d.createBuffer({size:4194300,usage:128});
    bad(() => d.createBuffer({size:4,usage:128}));
    d.destroy(); bad(() => d.createCommandEncoder());
  `);
  assert.equal(r.calls(), 0);
});
test('restored native handles rejected', async () => {
  const r = runtime();
  await r.run('globalThis.device = await (await navigator.gpu.requestAdapter()).requestDevice();');
  r.stale();
  await assert.rejects(r.run('device.createBuffer({size:4,usage:128});'), /stale/);
});
test('offscreen limits and unsupported rendering options fail before native dispatch', async () => {
 const r=runtime();
 await r.run(`
 const d=await (await navigator.gpu.requestAdapter()).requestDevice();
 const bad=f=>{let failed=false;try{f();}catch{failed=true;}if(!failed)throw Error('invalid render descriptor accepted');};
 bad(()=>d.createTexture({size:[1025,64],format:'rgba8unorm',usage:17}));
 bad(()=>d.createTexture({size:[64,64],format:'rgba16float',usage:17}));
 bad(()=>d.createTexture({size:[64,64],format:'rgba8unorm',usage:17,sampleCount:4}));
 bad(()=>d.createBuffer({size:4,usage:GPUBufferUsage.UNIFORM|GPUBufferUsage.STORAGE}));
 bad(()=>d.createBuffer({size:65540,usage:GPUBufferUsage.UNIFORM}));
 const t=d.createTexture({size:[64,64],format:'rgba8unorm',usage:17});
 const e=d.createCommandEncoder();
 bad(()=>e.beginRenderPass({colorAttachments:[{view:t.createView(),loadOp:'load',storeOp:'store'}]}));
 d.destroy();bad(()=>t.createView());
 `);
 assert.equal(r.calls(),0);
});

test('resident identities stay stable across native copies; host writes advance revisions', async () => {
 const r=runtime();
 await r.run(`
 const d=await (await navigator.gpu.requestAdapter()).requestDevice();
 const b=d.createBuffer({size:4,usage:140});
 const p=d.createComputePipeline({layout:'auto',compute:{module:d.createShaderModule({code:''})}});
 const g=d.createBindGroup({layout:p.getBindGroupLayout(0),entries:[{binding:0,resource:{buffer:b}}]});
 async function submit(group=g,copy){const e=d.createCommandEncoder(),p1=e.beginComputePass();p1.setPipeline(p);p1.setBindGroup(0,group);p1.dispatchWorkgroups(1);p1.end();if(copy)e.copyBufferToBuffer(b,0,copy,0,4);d.queue.submit([e.finish()]);await d.queue.onSubmittedWorkDone();}
 d.queue.writeBuffer(b,0,new Uint32Array([3]));await submit();await submit();
 const c=d.createBuffer({size:4,usage:140});
 const cg=d.createBindGroup({layout:p.getBindGroupLayout(0),entries:[{binding:0,resource:{buffer:c}}]});
 d.queue.writeBuffer(b,0,new Uint32Array([7]));await submit(g,c);await submit(cg);
 b.destroy();let rejected=false;try{await submit();}catch{rejected=true;}if(!rejected)throw Error('destroyed buffer accepted');
 d.destroy();
 `);
 assert.equal(r.jobs.length,4);
 assert.deepEqual(r.jobs.map(j=>j.refs[0].revision),[1,1,2,0]);
 assert.equal(new Set(r.jobs.slice(0,3).map(j=>j.refs[0].id)).size,1);
 assert.deepEqual(r.jobs[3].buffers[0],[14,0,0,0]);
});
test('command recording freezes dispatch state and rejects invalid pass lifetimes', async () => {
 const r=runtime();
 await r.run(`
 const d=await (await navigator.gpu.requestAdapter()).requestDevice();
 const a=d.createBuffer({size:4,usage:140}),b=d.createBuffer({size:4,usage:140});
 const p=d.createComputePipeline({layout:'auto',compute:{module:d.createShaderModule({code:'one'})}});
 const p2=d.createComputePipeline({layout:'auto',compute:{module:d.createShaderModule({code:'two'})}});
 const g=p=>d.createBindGroup({layout:p.getBindGroupLayout(0),entries:[{binding:0,resource:{buffer:a}}]});const g1=g(p),g2=g(p2);
 const bad=f=>{let failed=false;try{f();}catch{failed=true;}if(!failed)throw Error('invalid command accepted');};
 const e=d.createCommandEncoder(),pass=e.beginComputePass();
 bad(()=>e.beginComputePass());bad(()=>e.copyBufferToBuffer(a,0,b,0,4));bad(()=>e.finish());
 pass.setPipeline(p);pass.setBindGroup(0,g1);pass.dispatchWorkgroups(1);pass.setPipeline(p2);pass.setBindGroup(0,g2);pass.dispatchWorkgroups(1);pass.end();
 bad(()=>pass.dispatchWorkgroups(1));bad(()=>pass.end());
 e.copyBufferToBuffer(a,0,b,0,4);const command=e.finish();bad(()=>e.beginComputePass());d.queue.submit([command]);await d.queue.onSubmittedWorkDone();bad(()=>d.queue.submit([command]));
 const other=await (await navigator.gpu.requestAdapter()).requestDevice();const foreign=other.createBuffer({size:4,usage:140});const e2=d.createCommandEncoder();bad(()=>e2.copyBufferToBuffer(foreign,0,b,0,4));
 d.destroy();
 `);
 assert.deepEqual(r.jobs[0].commands.map(c=>c.kind),['compute','compute','copy']);
 assert.deepEqual(r.jobs[0].commands.slice(0,2).map(c=>c.code),['one','two']);
});

test('destroyed pending resource does not leave surviving CPU mirrors stale', async () => {
 const r=runtime();
 await r.run(`
 const d=await (await navigator.gpu.requestAdapter()).requestDevice();
 const a=d.createBuffer({size:4,usage:140}),b=d.createBuffer({size:4,usage:140}),out=d.createBuffer({size:4,usage:9});
 d.queue.writeBuffer(a,0,new Uint32Array([3]));
 const p=d.createComputePipeline({layout:'auto',compute:{module:d.createShaderModule({code:''})}}),g=d.createBindGroup({layout:p.getBindGroupLayout(0),entries:[{binding:0,resource:{buffer:a}}]});
 const e=d.createCommandEncoder(),pass=e.beginComputePass();pass.setPipeline(p);pass.setBindGroup(0,g);pass.dispatchWorkgroups(1);pass.end();e.copyBufferToBuffer(a,0,b,0,4);d.queue.submit([e.finish()]);a.destroy();
 let rejected=false;try{await d.queue.onSubmittedWorkDone();}catch{rejected=true;}if(!rejected)throw Error('destruction not reported');
 const copy=d.createCommandEncoder();copy.copyBufferToBuffer(b,0,out,0,4);d.queue.submit([copy.finish()]);await out.mapAsync(1);if(new Uint32Array(out.getMappedRange())[0]!==6)throw Error('surviving mirror stale');out.unmap();d.destroy();
 `);
});
test('binding and copy ranges reject overflow, misalignment and aliases', async () => {
 const r=runtime();
 await r.run(`
 const d=await (await navigator.gpu.requestAdapter()).requestDevice();
 const a=d.createBuffer({size:512,usage:140}),b=d.createBuffer({size:256,usage:140});
 const p=d.createComputePipeline({layout:'auto',compute:{module:d.createShaderModule({code:''})}});
 const bind=(offset,size)=>d.createBindGroup({layout:p.getBindGroupLayout(0),entries:[{binding:0,resource:{buffer:a,offset,size}}]});
 const bad=f=>{let failed=false;try{f();}catch{failed=true;}if(!failed)throw Error('unsafe range accepted');};
 for(const [offset,size] of [[4,4],[256,260],[0,0],[0,3],[-256,4],[Number.MAX_SAFE_INTEGER,4]])bad(()=>bind(offset,size));
 bind(256,256);const e=d.createCommandEncoder();
 for(const args of [[a,0,a,4,4],[a,2,b,0,4],[a,0,b,2,4],[a,0,b,0,3],[a,0,b,0,0],[a,512,b,0,4],[a,0,b,256,4],[a,Number.MAX_SAFE_INTEGER,b,0,4]])bad(()=>e.copyBufferToBuffer(...args));
 e.copyBufferToBuffer(a,508,b,252,4);d.queue.submit([e.finish()]);await d.queue.onSubmittedWorkDone();d.destroy();
 `);
 assert.equal(r.jobs[0].commands[0].source_offset,508);
 assert.equal(r.jobs[0].commands[0].destination_offset,252);
 assert.equal(r.jobs[0].commands[0].size,4);
});

test('capabilities are immutable and adapter/device requests are validated', async () => {
 const r=runtime();
 await r.run(`
 const rejects=async f=>{let error;try{await f();}catch(e){error=e;}if(!error)throw Error('invalid capability request accepted');};
 await rejects(()=>navigator.gpu.requestAdapter({powerPreference:'fastest'}));
 await rejects(()=>navigator.gpu.requestAdapter({forceFallbackAdapter:'false'}));
 const a=await navigator.gpu.requestAdapter({powerPreference:'high-performance',forceFallbackAdapter:false});
 if(a.features.size!==0||a.features.has('shader-f16')||a.features.add!==undefined||Reflect.set(a.limits,'maxBufferSize',1))throw Error('mutable/unbacked capabilities');
 await rejects(()=>a.requestDevice({requiredFeatures:['shader-f16']}));
 await rejects(()=>a.requestDevice({requiredLimits:{maxBufferSize:4}}));
 const d=await a.requestDevice({requiredLimits:a.limits,requiredFeatures:[]});d.destroy();await rejects(()=>a.requestAdapterInfo());
 `);
 assert.equal(r.requests.length,1);assert.deepEqual(r.requests[0],{powerPreference:'high-performance',forceFallbackAdapter:false});
});

test('native error delivery preserves classes and revalidates after handler destruction', async()=>{
 const r=runtime();await r.run(`globalThis.d=await(await navigator.gpu.requestAdapter()).requestDevice();globalThis.seen=[];d.onuncapturederror=e=>{seen.push([e.error.name,e.error instanceof GPUValidationError,e.target===d]);d.destroy();};`);
 r.emit({errors:[{kind:'validation',message:'first'},{kind:'internal',message:'late'}]});
 await r.run(`await d.lost;if(seen.length!==1||!seen[0][1]||!seen[0][2])throw Error('delivery after destroy or wrong class');if((await d.lost).reason!=='destroyed')throw Error('destroy loss missing');`);
});
test('overflow rejects loss promise and terminates without fabricated native loss',async()=>{
 const r=runtime();await r.run(`globalThis.d=await(await navigator.gpu.requestAdapter()).requestDevice();globalThis.seen=0;d.onuncapturederror=()=>seen++;`);
 r.emit({overflow:true,errors:[],loss:null});
 await r.run(`let rejected=false;try{await d.lost;}catch(e){rejected=e.message.includes('overflow');}if(!rejected||seen)throw Error('overflow not terminal');let dead=false;try{d.createBuffer({size:4,usage:128});}catch{dead=true;}if(!dead)throw Error('overflow device alive');`);
});
test('stale drained events do not reach a handler',async()=>{
 const r=runtime();await r.run(`globalThis.d=await(await navigator.gpu.requestAdapter()).requestDevice();globalThis.seen=0;d.onuncapturederror=()=>seen++;`);
 r.stale();r.emit({errors:[{kind:'validation',message:'stale'}]});
 await r.run(`try{await d.lost;}catch{}if(seen)throw Error('stale event delivered');`);
});
test('native loss resolves once and OOM/internal event classes remain distinct',async()=>{
 const r=runtime();await r.run(`globalThis.d=await(await navigator.gpu.requestAdapter()).requestDevice();globalThis.seen=[];d.onuncapturederror=e=>seen.push(e.error.name);`);
 r.emit({errors:[{kind:'out-of-memory',message:'oom'},{kind:'internal',message:'internal'}],loss:{kind:'unknown',message:'native'}});
 await r.run(`if((await d.lost).reason!=='unknown'||seen.join(',')!=='GPUOutOfMemoryError,GPUInternalError')throw Error('native classes/loss mismatch');`);
});

for(const drained of [false,true]) for(const overflow of [false,true]) {
 test(`${drained?'drained':'queued'} ${overflow?'overflow':'native loss'} survives destroy`,async()=>{
  const r=runtime();await r.run(`globalThis.d=await(await navigator.gpu.requestAdapter()).requestDevice();`);
  const event={errors:[],overflow,loss:overflow?null:{kind:'unknown',message:'earlier native loss'}};
  if(drained)r.emit(event);else r.queue(event);
  // Destroy synchronously, before the already-resolved receiver's continuation.
  await r.run(`d.destroy();d.destroy();let value;try{value=await d.lost;}catch(e){value=e.message;}if(${overflow} ? typeof value!=='string'||!value.includes('overflow') : value.reason!=='unknown'||value.message!=='earlier native loss')throw Error('earlier terminal outcome lost');`);
 });
}
