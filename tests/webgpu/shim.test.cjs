// Fast state-machine tests with a fake native boundary; real GPU coverage is in server/tests/webgpu.rs.
const { test } = require('node:test');
const assert = require('node:assert/strict');
const vm = require('node:vm');
const fs = require('node:fs');
const source = fs.readFileSync('server/src/engine/web_compat/webgpu.js', 'utf8');
function runtime(transfer, options = {}) {
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
      if(options.before)await options.before(job,calls);
      const data=job.buffers.map(b=>Uint8Array.from(b));
      for(const c of job.commands){
        if(c.kind==='copy')data[c.destination].set(data[c.source].subarray(c.source_offset,c.source_offset+c.size),c.destination_offset);
        else {const words=new Uint32Array(data[c.bindings[0].buffer].buffer,c.bindings[0].offset,c.bindings[0].size/4);for(let i=0;i<words.length;i++)words[i]*=2;}
      }
      if(options.after)await options.after(job,calls);
      return JSON.stringify(data.map(b=>Array.from(b)));
    }
  } } } });
  if(transfer){c.__transfer=transfer;vm.runInContext('ArrayBuffer.prototype.transfer=__transfer;delete globalThis.__transfer;',c);}
  vm.runInContext(source, c);
  return { run: code => vm.runInContext('(async () => {' + code + '})()', c), valid:()=>valid, stale: () => { valid = false; }, calls: () => calls, queue: value=>{if(!outcome.loss&&!outcome.overflow){outcome={loss:value.loss??null,overflow:!!value.overflow};}}, emit: value=>{if(!outcome.loss&&!outcome.overflow){outcome={loss:value.loss??null,overflow:!!value.overflow};}deliver(JSON.stringify(value));}, jobs, requests };
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
    bad(() => d.createBuffer({size: 4,usage:128,mappedAtCreation:1}));
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
 await d.queue.onSubmittedWorkDone();
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
test('mapped initialization zeroes, detaches, uploads and keeps usage immutable',async()=>{
 const r=runtime();await r.run(`
 const d=await(await navigator.gpu.requestAdapter()).requestDevice();const bad=f=>{let failed=false;try{f();}catch{failed=true;}if(!failed)throw Error('invalid mapping accepted');};
 const b=d.createBuffer({size:16,usage:132,mappedAtCreation:true});if(b.mapState!=='mapped'||Reflect.set(b,'usage',140))throw Error('initial state/usage');
 bad(()=>b.getMappedRange(4,4));bad(()=>b.getMappedRange(0,20));
 const range=b.getMappedRange();if(new Uint32Array(range).some(x=>x))throw Error('not zeroed');new Uint32Array(range).set([1,0,3]);bad(()=>b.getMappedRange());
 const p=d.createComputePipeline({layout:'auto',compute:{module:d.createShaderModule({code:'mock'})}}),g=d.createBindGroup({layout:p.getBindGroupLayout(0),entries:[{binding:0,resource:{buffer:b}}]});
 const e=d.createCommandEncoder(),pass=e.beginComputePass();pass.setPipeline(p);pass.setBindGroup(0,g);pass.dispatchWorkgroups(1);pass.end();const out=d.createBuffer({size:16,usage:9});e.copyBufferToBuffer(b,0,out,0,16);const c=e.finish();bad(()=>d.queue.submit([c]));
 b.unmap();if(range.byteLength!==0||b.mapState!=='unmapped')throw Error('unmap did not detach');bad(()=>b.getMappedRange());b.unmap();d.queue.submit([c]);await out.mapAsync(1);if(Array.from(new Uint32Array(out.getMappedRange())).join()!=='2,0,6,0')throw Error('initialized bytes lost');d.destroy();
 `);assert.equal(r.jobs[0].refs[0].revision,1);assert.equal(r.jobs[0].usages[0],132);
});
test('mapped ranges detach on buffer/device destruction; detached init and invalid flags fail',async()=>{
 const r=runtime();await r.run(`
 const d=await(await navigator.gpu.requestAdapter()).requestDevice();const bad=f=>{let failed=false;try{f();}catch{failed=true;}if(!failed)throw Error('invalid mapping accepted');};
 for(const usage of [3,10,130])bad(()=>d.createBuffer({size:16,usage,mappedAtCreation:true}));
 bad(()=>d.createBuffer({size:6,usage:6,mappedAtCreation:true}));
 const a=d.createBuffer({size:16,usage:6,mappedAtCreation:true}),ar=a.getMappedRange();a.destroy();if(ar.byteLength)throw Error('buffer destroy mapping alive');bad(()=>a.unmap());
 const b=d.createBuffer({size:16,usage:6,mappedAtCreation:true}),br=b.getMappedRange();structuredClone(br,{transfer:[br]});bad(()=>b.unmap());b.destroy();
 const c=d.createBuffer({size:16,usage:6,mappedAtCreation:true}),cr=c.getMappedRange();d.destroy();if(cr.byteLength||c.mapState!=='unmapped')throw Error('device destroy mapping alive');
 `);
});
test('mapped initialization shares unchanged cumulative allocation budget',async()=>{
 const r=runtime();await r.run(`
 const d=await(await navigator.gpu.requestAdapter()).requestDevice();const b=d.createBuffer({size:4194304,usage:6,mappedAtCreation:true});const range=b.getMappedRange();if(range.byteLength!==4194304)throw Error('range size');b.unmap();b.destroy();let failed=false;try{d.createBuffer({size:4,usage:6,mappedAtCreation:true});}catch{failed=true;}if(!failed)throw Error('allocation budget refunded');d.destroy();
 `);
});

for(const shadow of ['throw','zero']) for(const action of ['unmap','buffer','device']) {
 test(`mapped ${shadow} byteLength cannot block ${action}`,async()=>{
  const r=runtime();await r.run(`
   const d=await(await navigator.gpu.requestAdapter()).requestDevice();
   const b=d.createBuffer({size:16,usage:6,mappedAtCreation:true}),range=b.getMappedRange();
   const length=Object.getOwnPropertyDescriptor(ArrayBuffer.prototype,'byteLength').get;
   new Uint32Array(range)[0]=42;
   Object.defineProperty(range,'byteLength',${shadow==='throw'?"{get(){throw Error('caller getter');}}":"{value:0}"});
   ${action==='unmap'?'b.unmap();':action==='buffer'?'b.destroy();':'d.destroy();'}
   if(length.call(range)!==0||b.mapState!=='unmapped')throw Error('range not detached');
   d.destroy();d.destroy();if((await d.lost).reason!=='destroyed')throw Error('loss not settled');
  `);assert.equal(r.valid(),false);
 });
}
test('mapping cleanup failure cannot bypass native teardown or loss settlement',async()=>{
 let attempts=0;
 const r=runtime(function(){attempts++;if(attempts===1)throw Error('injected cleanup failure');return ArrayBuffer.prototype.transfer.call(this);});
 await r.run(`
  const d=await(await navigator.gpu.requestAdapter()).requestDevice();
  const a=d.createBuffer({size:16,usage:6,mappedAtCreation:true});a.getMappedRange();
  const b=d.createBuffer({size:16,usage:6,mappedAtCreation:true}),range=b.getMappedRange();
  d.destroy();d.destroy();let error;try{await d.lost;}catch(e){error=e;}
  if(!error?.message.includes('injected cleanup failure')||range.byteLength||a.mapState!=='unmapped'||b.mapState!=='unmapped')throw Error('cleanup/settlement skipped');
 `);assert.equal(r.valid(),false);assert.equal(attempts,2);
});

test('FIFO retains destroyed staging and maps after all admitted uses; later write/remap works',async()=>{
 const r=runtime();await r.run(`
 const d=await(await navigator.gpu.requestAdapter()).requestDevice(),out=d.createBuffer({size:4,usage:9}),b=d.createBuffer({size:4,usage:12});
 const copy=(a,z)=>{const e=d.createCommandEncoder();e.copyBufferToBuffer(a,0,z,0,4);return e.finish();};
 for(const n of [11,22]){const a=d.createBuffer({size:4,usage:6,mappedAtCreation:true});new Uint32Array(a.getMappedRange())[0]=n;a.unmap();d.queue.submit([copy(a,out)]);a.destroy();}
 await out.mapAsync(1);if(new Uint32Array(out.getMappedRange())[0]!==22)throw Error('FIFO/mapping mismatch');out.unmap();
 d.queue.writeBuffer(b,0,new Uint32Array([33]));d.queue.submit([copy(b,out)]);await out.mapAsync(1);if(new Uint32Array(out.getMappedRange())[0]!==33)throw Error('later write/remap mismatch');out.unmap();d.destroy();
 `);assert.equal(r.calls(),3);
});
test('work and map fences capture fixed prefixes, pending map does not invalidate prior jobs',async()=>{
 const gates=[];const r=runtime(undefined,{before:()=>new Promise(resolve=>gates.push(resolve))});
 await r.run(`
 globalThis.d=await(await navigator.gpu.requestAdapter()).requestDevice();globalThis.a=d.createBuffer({size:4,usage:12});globalThis.b=d.createBuffer({size:4,usage:12});globalThis.out=d.createBuffer({size:4,usage:9});
 globalThis.copy=(a,z)=>{const e=d.createCommandEncoder();e.copyBufferToBuffer(a,0,z,0,4);return e.finish();};d.queue.writeBuffer(a,0,new Uint32Array([7]));
 d.queue.submit([copy(a,b)]);globalThis.first=d.queue.onSubmittedWorkDone();d.queue.submit([copy(b,out)]);globalThis.secondDone=false;globalThis.second=d.queue.onSubmittedWorkDone().then(()=>secondDone=true);
 globalThis.mapped=out.mapAsync(1);d.queue.submit([copy(a,b)]);globalThis.last=d.queue.onSubmittedWorkDone();last.catch(()=>{});
 let bad=false;try{d.queue.writeBuffer(out,0,new Uint32Array([9]));}catch{bad=true;}if(!bad)throw Error('pending-map write accepted');
 `);assert.equal(r.calls(),1);gates[0]();await r.run('await first;if(secondDone)throw Error("prefix waited incorrectly");');assert.equal(r.calls(),2);
 gates[1]();await r.run('await mapped;await second;if(new Uint32Array(out.getMappedRange())[0]!==7)throw Error("map prefix data");out.unmap();');assert.equal(r.calls(),3);
 await r.run('d.destroy();await Promise.allSettled([last]);await first;');gates[2]();assert.equal(r.valid(),false);
});
test('middle failure cancels suffix, preserves earlier fence and rejects future submission',async()=>{
 const r=runtime(undefined,{before:(_,n)=>{if(n===2)throw Error('policy denied middle');}});
 await r.run(`
 const d=await(await navigator.gpu.requestAdapter()).requestDevice(),a=d.createBuffer({size:4,usage:12}),b=d.createBuffer({size:4,usage:12});
 const copy=()=>{const e=d.createCommandEncoder();e.copyBufferToBuffer(a,0,b,0,4);return e.finish();};
 const fences=[];for(let i=0;i<3;i++){d.queue.submit([copy()]);const f=d.queue.onSubmittedWorkDone();f.catch(()=>{});fences.push(f);}
 const result=await Promise.allSettled(fences);if(result.map(x=>x.status).join()!=='fulfilled,rejected,rejected')throw Error('suffix not cancelled');
 if(!result[1].reason.message.includes('policy denied middle')||result[1].reason!==result[2].reason)throw Error('failure changed');
 let failed=false;try{d.queue.submit([{}]);}catch{failed=true;}if(!failed)throw Error('failed queue reused');try{await d.lost;}catch{}d.destroy();
 `);assert.equal(r.calls(),2);assert.equal(r.valid(),false);
});
for(const phase of ['before','after'])test(`destroy during native ${phase} settles every fence before worker returns`,async()=>{
 let release;const r=runtime(undefined,{[phase]:()=>new Promise(resolve=>release=resolve)});
 await r.run(`
 globalThis.d=await(await navigator.gpu.requestAdapter()).requestDevice();const a=d.createBuffer({size:4,usage:12}),b=d.createBuffer({size:4,usage:9});
 const copy=()=>{const e=d.createCommandEncoder();e.copyBufferToBuffer(a,0,b,0,4);return e.finish();};
 d.queue.submit([copy()]);globalThis.first=d.queue.onSubmittedWorkDone();first.catch(()=>{});d.queue.submit([copy()]);globalThis.last=d.queue.onSubmittedWorkDone();last.catch(()=>{});globalThis.mapping=b.mapAsync(1);mapping.catch(()=>{});
 d.destroy();d.destroy();if((await d.lost).reason!=='destroyed')throw Error('loss settlement');
 if((await Promise.allSettled([first,last,mapping])).some(x=>x.status!=='rejected'))throw Error('waiter survived teardown');
 `);assert.equal(r.valid(),false);assert.equal(r.calls(),1);release();await r.run('await Promise.resolve();');assert.equal(r.calls(),1);
});
test('restored token before readback cancels queued suffix',async()=>{
 let release;const r=runtime(undefined,{after:()=>new Promise(resolve=>release=resolve)});
 await r.run(`globalThis.d=await(await navigator.gpu.requestAdapter()).requestDevice();const a=d.createBuffer({size:4,usage:12}),b=d.createBuffer({size:4,usage:12});for(let i=0;i<2;i++){const e=d.createCommandEncoder();e.copyBufferToBuffer(a,0,b,0,4);d.queue.submit([e.finish()]);}globalThis.f=d.queue.onSubmittedWorkDone();f.catch(()=>{});`);
 r.stale();release();await assert.rejects(r.run('await f;'),/stale/);assert.equal(r.calls(),1);
});
test('FIFO charges cumulative 32 units without refund',async()=>{
 const r=runtime();await r.run(`
 const d=await(await navigator.gpu.requestAdapter()).requestDevice(),a=d.createBuffer({size:4,usage:12}),b=d.createBuffer({size:4,usage:12});
 const copy=()=>{const e=d.createCommandEncoder();e.copyBufferToBuffer(a,0,b,0,4);return e.finish();};
 for(let i=0;i<32;i++)d.queue.submit([copy()]);const c=copy();let failed=false;try{d.queue.submit([c]);}catch{failed=true;}if(!failed)throw Error('33rd outstanding admitted');
 await d.queue.onSubmittedWorkDone();failed=false;try{d.queue.submit([c]);}catch(e){failed=e.message.includes('work limit');}if(!failed)throw Error('work budget refunded');d.destroy();
 `);assert.equal(r.calls(),32);
});
test('UTF-8 aggregate metadata bound rejects before command consumption and releases after completion',async()=>{
 let release;const r=runtime(undefined,{before:(_,n)=>n===1?new Promise(resolve=>release=resolve):undefined});
 await r.run(`
 globalThis.d=await(await navigator.gpu.requestAdapter()).requestDevice();const b=d.createBuffer({size:4,usage:128});
 const p=d.createComputePipeline({layout:'auto',compute:{module:d.createShaderModule({code:'λ'.repeat(20000)})}}),g=d.createBindGroup({layout:p.getBindGroupLayout(0),entries:[{binding:0,resource:{buffer:b}}]});
 const command=()=>{const e=d.createCommandEncoder(),pass=e.beginComputePass();pass.setPipeline(p);pass.setBindGroup(0,g);pass.dispatchWorkgroups(1);pass.end();return e.finish();};
 d.queue.submit([command()]);globalThis.c=command();let failed=false;try{d.queue.submit([c]);}catch(e){failed=e.message.includes('metadata');}if(!failed)throw Error('UTF8 metadata unbounded');globalThis.first=d.queue.onSubmittedWorkDone();
 `);assert.equal(r.calls(),1);release();await r.run('await first;d.queue.submit([c]);await d.queue.onSubmittedWorkDone();d.destroy();');assert.equal(r.calls(),2);
});
test('aggregate reference cap and invalid mapping rejection do not consume commands/work',async()=>{
 const r=runtime();await r.run(`
 const d=await(await navigator.gpu.requestAdapter()).requestDevice();const bs=Array.from({length:16},()=>d.createBuffer({size:4,usage:12}));
 const command=()=>{const e=d.createCommandEncoder();for(let i=0;i<16;i+=2)e.copyBufferToBuffer(bs[i],0,bs[i+1],0,4);return e.finish();};
 for(let i=0;i<8;i++)d.queue.submit([command()]);const c=command();let failed=false;try{d.queue.submit([c]);}catch(e){failed=e.message.includes('reference');}if(!failed)throw Error('reference limit missing');
 await d.queue.onSubmittedWorkDone();d.queue.submit([c]);await d.queue.onSubmittedWorkDone();
 const a=d.createBuffer({size:4,usage:6,mappedAtCreation:true}),e=d.createCommandEncoder();e.copyBufferToBuffer(a,0,bs[1],0,4);const pending=e.finish();
 for(let i=0;i<40;i++){let bad=false;try{d.queue.submit([pending]);}catch{bad=true;}if(!bad)throw Error('mapped admitted');}a.unmap();d.queue.submit([pending]);await d.queue.onSubmittedWorkDone();d.destroy();
 `);assert.equal(r.calls(),10);
});
for(const overflow of [false,true])test(`queued ${overflow?'overflow':'native loss'} cancels fences with authoritative terminal cause`,async()=>{
 let release;const r=runtime(undefined,{after:()=>new Promise(resolve=>release=resolve)});
 await r.run(`globalThis.d=await(await navigator.gpu.requestAdapter()).requestDevice();const a=d.createBuffer({size:4,usage:12}),b=d.createBuffer({size:4,usage:12});globalThis.fences=[];for(let i=0;i<2;i++){const e=d.createCommandEncoder();e.copyBufferToBuffer(a,0,b,0,4);d.queue.submit([e.finish()]);const f=d.queue.onSubmittedWorkDone();f.catch(()=>{});fences.push(f);}`);
 r.queue({overflow,loss:overflow?null:{kind:'unknown',message:'authoritative native loss'}});
 await r.run(`d.destroy();const results=await Promise.allSettled(fences);if(results.some(x=>x.status!=='rejected'||!x.reason.message.includes(${JSON.stringify(overflow?'overflow':'authoritative native loss')})))throw Error('terminal cause lost');let value;try{value=await d.lost;}catch(e){value=e;}if(${overflow}?!value.message.includes('overflow'):value.reason!=='unknown')throw Error('loss precedence');`);
 release();assert.equal(r.calls(),1);
});

test('queued writes snapshot selected intrinsic view bytes despite caller mutation and detachment',async()=>{
 let release;const r=runtime(undefined,{before:(_,n)=>n===1?new Promise(resolve=>release=resolve):undefined});
 await r.run(`
 globalThis.d=await(await navigator.gpu.requestAdapter()).requestDevice();globalThis.a=d.createBuffer({size:16,usage:12});globalThis.out=d.createBuffer({size:16,usage:9});
 const copy=()=>{const e=d.createCommandEncoder();e.copyBufferToBuffer(a,0,out,0,16);return e.finish();};
 d.queue.submit([copy()]);const input=new Uint32Array([1,2,3,4]),backing=input.buffer;
 for(const key of ['buffer','byteOffset','byteLength','BYTES_PER_ELEMENT'])Object.defineProperty(input,key,{get(){throw Error('caller getter '+key);}});
 d.queue.writeBuffer(a,4,input,1,2);input[1]=99;structuredClone(backing,{transfer:[backing]});
 d.queue.submit([copy()]);globalThis.mapped=out.mapAsync(1);
 `);assert.equal(r.calls(),1);assert.deepEqual(r.jobs[0].buffers[0],Array(16).fill(0));release();
 await r.run('await mapped;if(Array.from(new Uint32Array(out.getMappedRange())).join()!=="0,2,3,0")throw Error("snapshot changed");out.unmap();d.destroy();');assert.equal(r.jobs[1].refs[0].revision,1);
});
test('partial queued writes preserve GPU readback and overlapping dispatch order',async()=>{
 const r=runtime();await r.run(`
 const d=await(await navigator.gpu.requestAdapter()).requestDevice(),a=d.createBuffer({size:16,usage:140}),out=d.createBuffer({size:16,usage:9});
 const p=d.createComputePipeline({layout:'auto',compute:{module:d.createShaderModule({code:'mock'})}}),g=d.createBindGroup({layout:p.getBindGroupLayout(0),entries:[{binding:0,resource:{buffer:a}}]});
 const compute=()=>{const e=d.createCommandEncoder(),pass=e.beginComputePass();pass.setPipeline(p);pass.setBindGroup(0,g);pass.dispatchWorkgroups(1);pass.end();d.queue.submit([e.finish()]);};
 d.queue.writeBuffer(a,0,new Uint32Array([1,2,3,4]));compute();d.queue.writeBuffer(a,4,new Uint32Array([10,20]));compute();d.queue.writeBuffer(a,8,new Uint32Array([7]));
 const e=d.createCommandEncoder();e.copyBufferToBuffer(a,0,out,0,16);d.queue.submit([e.finish()]);await out.mapAsync(1);if(Array.from(new Uint32Array(out.getMappedRange())).join()!=='4,20,7,16')throw Error('write/dispatch ordering');out.unmap();d.destroy();
 `);assert.deepEqual(r.jobs.map(j=>j.refs[0].revision),[1,2,3]);
});
test('admitted writes survive destination destruction and reject future dead-wrapper use',async()=>{
 let release;const r=runtime(undefined,{before:(_,n)=>n===1?new Promise(resolve=>release=resolve):undefined});
 await r.run(`
 globalThis.d=await(await navigator.gpu.requestAdapter()).requestDevice();const a=d.createBuffer({size:4,usage:12}),out=d.createBuffer({size:4,usage:9});globalThis.out=out;
 const copy=()=>{const e=d.createCommandEncoder();e.copyBufferToBuffer(a,0,out,0,4);return e.finish();};d.queue.submit([copy()]);d.queue.writeBuffer(a,0,new Uint32Array([37]));d.queue.submit([copy()]);a.destroy();
 let bad=false;try{d.queue.writeBuffer(a,0,new Uint32Array([99]));}catch{bad=true;}if(!bad)throw Error('dead upload accepted');globalThis.mapped=out.mapAsync(1);
 `);release();await r.run('await mapped;if(new Uint32Array(out.getMappedRange())[0]!==37)throw Error("retained upload lost");out.unmap();d.destroy();');
});
test('map fence includes queued upload but excludes subsequent unrelated work',async()=>{
 const gates=[];const r=runtime(undefined,{before:()=>new Promise(resolve=>gates.push(resolve))});
 await r.run(`
 globalThis.d=await(await navigator.gpu.requestAdapter()).requestDevice();const a=d.createBuffer({size:4,usage:12}),b=d.createBuffer({size:4,usage:12});globalThis.out=d.createBuffer({size:4,usage:9});
 const copy=(z)=>{const e=d.createCommandEncoder();e.copyBufferToBuffer(a,0,z,0,4);return e.finish();};d.queue.submit([copy(out)]);d.queue.writeBuffer(out,0,new Uint32Array([51]));globalThis.mapped=out.mapAsync(1);d.queue.submit([copy(b)]);globalThis.last=d.queue.onSubmittedWorkDone();last.catch(()=>{});
 `);gates[0]();await r.run('await mapped;if(new Uint32Array(out.getMappedRange())[0]!==51)throw Error("upload absent from prefix");out.unmap();');assert.equal(r.calls(),2);await r.run('d.destroy();await Promise.allSettled([last]);');gates[1]();
});
test('upload staging uses cumulative allocation budget with no refund',async()=>{
 const r=runtime();await r.run(`
 const d=await(await navigator.gpu.requestAdapter()).requestDevice(),b=d.createBuffer({size:2097152,usage:8}),source=new Uint8Array(1048576);
 d.queue.writeBuffer(b,0,source);d.queue.writeBuffer(b,0,source);await d.queue.onSubmittedWorkDone();
 let failed=false;try{d.queue.writeBuffer(b,0,new Uint32Array([1]));}catch(e){failed=e.message.includes('staging limit');}if(!failed)throw Error('staging budget refunded');b.destroy();failed=false;try{d.createBuffer({size:4,usage:8});}catch{failed=true;}if(!failed)throw Error('allocation charge lost');d.destroy();
 `);assert.equal(r.calls(),0);
});
test('each upload consumes one cumulative work unit even without native dispatch',async()=>{
 const r=runtime();await r.run(`
 const d=await(await navigator.gpu.requestAdapter()).requestDevice(),b=d.createBuffer({size:4,usage:8});for(let i=0;i<32;i++)d.queue.writeBuffer(b,0,new Uint32Array([i]));await d.queue.onSubmittedWorkDone();let failed=false;try{d.queue.writeBuffer(b,0,new Uint32Array([33]));}catch(e){failed=e.message.includes('work limit');}if(!failed)throw Error('upload work budget refunded');d.destroy();
 `);assert.equal(r.calls(),0);
});
for(const cancel of ['denied','destroy','loss','overflow'])test(`queued upload snapshot is cancelled on ${cancel}`,async()=>{
 let release;const r=runtime(undefined,{before:()=>new Promise((resolve,reject)=>release=cancel==='denied'?()=>reject(Error('policy denied')):resolve)});
 await r.run(`
 globalThis.d=await(await navigator.gpu.requestAdapter()).requestDevice();const a=d.createBuffer({size:4,usage:12}),b=d.createBuffer({size:4,usage:12});const e=d.createCommandEncoder();e.copyBufferToBuffer(a,0,b,0,4);d.queue.submit([e.finish()]);d.queue.writeBuffer(a,0,new Uint32Array([99]));globalThis.f=d.queue.onSubmittedWorkDone();f.catch(()=>{});
 `);
 if(cancel==='denied')release();else {if(cancel==='loss')r.queue({loss:{kind:'unknown',message:'native loss'}});if(cancel==='overflow')r.queue({overflow:true});await r.run('d.destroy();');release();}
 await assert.rejects(r.run('await f;'));assert.equal(r.calls(),1);assert.deepEqual(r.jobs[0].buffers[0],[0,0,0,0]);assert.equal(r.valid(),false);
});
test('BufferSource units, DataView offsets and zero-length uploads validate before admission',async()=>{
 const r=runtime();await r.run(`
 const d=await(await navigator.gpu.requestAdapter()).requestDevice(),b=d.createBuffer({size:16,usage:12}),out=d.createBuffer({size:16,usage:9});
 const source=new Uint32Array([10,20,30,40]);d.queue.writeBuffer(b,0,new DataView(source.buffer,4,8),4,4);d.queue.writeBuffer(b,4,source.buffer,8,4);d.queue.writeBuffer(b,16,new ArrayBuffer(0));
 const bad=f=>{let failed=false;try{f();}catch{failed=true;}if(!failed)throw Error('invalid upload accepted');};
 bad(()=>d.queue.writeBuffer(b,2,source));bad(()=>d.queue.writeBuffer(b,0,source,4,1));bad(()=>d.queue.writeBuffer(b,0,new Uint8Array(3)));const detached=new ArrayBuffer(4);structuredClone(detached,{transfer:[detached]});bad(()=>d.queue.writeBuffer(b,0,detached));
 const e=d.createCommandEncoder();e.copyBufferToBuffer(b,0,out,0,16);d.queue.submit([e.finish()]);await out.mapAsync(1);if(Array.from(new Uint32Array(out.getMappedRange())).join()!=='30,30,0,0')throw Error('BufferSource units wrong');out.unmap();d.destroy();
 `);
});
