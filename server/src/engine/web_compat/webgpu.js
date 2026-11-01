// Bounded compute and offscreen WebGPU subset; see site-docs/how-to/webgpu.md.
(() => {
  const { op_gpu_request: request, op_gpu_batch: batch,
          op_gpu_destroy: destroy, op_gpu_check: check, op_gpu_render: render, op_gpu_describe: describe, op_gpu_validate_device: validateDevice, op_gpu_events: receiveEvents, op_gpu_event_check: eventCheck } = Deno.core.ops;
  const unrefOp = Deno.core.unrefOpPromise;
  class GPUValidationError extends Error { constructor(message){super(message);this.name="GPUValidationError";} }
  class GPUOutOfMemoryError extends Error { constructor(message){super(message);this.name="GPUOutOfMemoryError";} }
  class GPUInternalError extends Error { constructor(message){super(message);this.name="GPUInternalError";} }
  const U = Object.freeze({ MAP_READ: 1, COPY_SRC: 4, COPY_DST: 8, VERTEX: 32, UNIFORM: 64, STORAGE: 128 });
  const buffers = new WeakMap(), shaders = new WeakMap(), pipelines = new WeakMap();
  const layouts = new WeakMap(), groups = new WeakMap(), commands = new WeakMap();
  const textures = new WeakMap(), views = new WeakMap(), renderPipelines = new WeakMap();
  function fail(message) { throw new Error('webgpu: ' + message); }
  function fields(d, allowed) {
    if (!d || typeof d !== 'object') throw new TypeError('descriptor required');
    for (const key of Object.keys(d)) if (key !== 'label' && !allowed.includes(key)) fail('unsupported option: ' + key);
  }
  function integer(n, max, name) {
    if (!Number.isSafeInteger(n) || n < 0 || n > max) fail('invalid ' + name);
    return n;
  }
  function owned(map, obj, device) {
    const v = map.get(obj);
    if (!v || v.device !== device) fail('invalid or cross-device object');
    return v;
  }
  function live(device) { if (device.dead) fail('device destroyed'); check(device.token); }
  function buffer(obj, device) {
    live(device);
    const b = owned(buffers, obj, device);
    if (b.dead) fail('buffer destroyed');
    return b;
  }
  async function requestAdapter(options = {}) {
    fields(options, ['powerPreference','forceFallbackAdapter']);
    if(options.powerPreference!==undefined&&!['low-power','high-performance'].includes(options.powerPreference))fail('invalid powerPreference');
    if(options.forceFallbackAdapter!==undefined&&typeof options.forceFallbackAdapter!=='boolean')fail('invalid forceFallbackAdapter');
    const token = await request(JSON.stringify({powerPreference:options.powerPreference,forceFallbackAdapter:options.forceFallbackAdapter??false}));
    if (!token) return null;
    const metadata=JSON.parse(describe(token));
    const limits=Object.freeze(metadata.limits),info=Object.freeze(metadata.info);
    const names=Object.freeze(metadata.features.slice());
    const features=Object.freeze({size:names.length,has:name=>names.includes(name),values:()=>names.values(),keys:()=>names.values(),entries:()=>names.map(n=>[n,n]).values(),[Symbol.iterator]:()=>names.values(),forEach(callback,thisArg){for(const n of names)callback.call(thisArg,n,n,features);}});
    const caps={limits,info,features};
    let used = false;
    return Object.freeze({limits,info,features,
      async requestAdapterInfo(){check(token);return info;},
      async requestDevice(options = {}) {
        fields(options, ['requiredFeatures','requiredLimits']);check(token);
        if (used) fail('one device per adapter');
        const requestedFeatures=options.requiredFeatures??[],requestedLimits=options.requiredLimits??{};
        if(!Array.isArray(requestedFeatures)||requestedFeatures.some(f=>typeof f!=='string'))fail('requiredFeatures must be an array of names');
        fields(requestedLimits,Object.keys(limits));
        for(const value of Object.values(requestedLimits))integer(value,Number.MAX_SAFE_INTEGER,'required limit');
        validateDevice(token,JSON.stringify({requiredFeatures:requestedFeatures,requiredLimits:requestedLimits}));
        used = true;
        return makeDevice(token,caps);
      }
    });
  }
  function makeDevice(token,caps) {
    const d = { token, dead: false, bytes: 0, objects: 0, submissions: 0, pending: false, tail: Promise.resolve() };
    let handler=null, resolveLost, rejectLost, lossSettled=false;
    const lost=new Promise((resolve,reject)=>{resolveLost=resolve;rejectLost=reject;});
    lost.catch(()=>{}); // Terminal bridge failure stays observable without an unhandled rejection.
    const settleLoss=(info)=>{if(!lossSettled){lossSettled=true;resolveLost(Object.freeze(info));}};
    const finish=(error)=>{
      if(d.dead)return;d.dead=true;
      let outcome;
      try{outcome=JSON.parse(destroy(token));}catch(nativeError){error=error??nativeError;}
      if(outcome?.overflow)error=new Error('webgpu: native error queue overflow; session terminated');
      else if(outcome?.loss){settleLoss({reason:outcome.loss.kind,message:outcome.loss.message});return;}
      if(error){if(!lossSettled){lossSettled=true;rejectLost(error);}return;}
      settleLoss({reason:'destroyed',message:'device.destroy()'});
    };
    const terminal=finish;
    const count = () => { live(d); if (++d.objects > 128) fail('object creation limit (128)'); };
    const api = {
      limits:caps.limits,features:caps.features,lost,
      get onuncapturederror(){return handler;},
      set onuncapturederror(value){live(d);if(value!==null&&typeof value!=='function')throw new TypeError('GPU error handler must be a function or null');handler=value;},
      destroy() { finish(); },
      createBuffer(desc) {
        fields(desc, ['size', 'usage', 'mappedAtCreation']); count();
        const size = integer(desc.size, 4 * 1024 * 1024, 'buffer size');
        const usage = integer(desc.usage, 237, 'buffer usage');
        if (!size || size % 4 || !usage || (usage & ~237)) fail('buffer size/alignment/usage');
        if ((usage & U.MAP_READ) && (usage & ~9)) fail('MAP_READ only supports COPY_DST');
        if ((usage & U.UNIFORM) && ((usage & U.STORAGE) || size>65536)) fail('uniform usage/size limit');
        if (desc.mappedAtCreation) fail('mappedAtCreation is unsupported');
        if (d.bytes + size > 4 * 1024 * 1024) fail('cumulative buffer limit (4 MiB)');
        d.bytes += size;
        const b = { id: d.objects, revision: 0, device: d, data: new Uint8Array(size), size, usage, dead: false, mapped: null, mapping: false, returned: false };
        const obj = Object.freeze({ size, usage,
          get mapState() { return b.mapped ? 'mapped' : b.mapping ? 'pending' : 'unmapped'; },
          destroy() { b.dead = true; b.mapped = null; b.data = null; },
          async mapAsync(mode, offset = 0, length = size) {
            buffer(obj, d);
            if (mode !== 1 || !(usage & U.MAP_READ) || offset !== 0 || length !== size) fail('only full-buffer MAP_READ is supported');
            if (b.mapped || b.mapping) fail('buffer already mapping');
            b.mapping = true;
            try { await d.tail; buffer(obj, d); b.mapped = b.data.slice().buffer; b.returned = false; }
            finally { b.mapping = false; }
          },
          getMappedRange(offset = 0, length = size) {
            buffer(obj, d);
            if (!b.mapped || b.returned || offset !== 0 || length !== size) fail('only one full mapped range is supported');
            b.returned = true; return b.mapped;
          },
          unmap() {
            buffer(obj, d);
            if (b.mapping) fail('map is pending');
            // Mapped ranges are detached, matching WebGPU's lifetime contract.
            if (b.mapped) structuredClone(b.mapped, { transfer: [b.mapped] });
            b.mapped = null;
          }
        });
        buffers.set(obj, b); return obj;
      },
      createShaderModule(desc) {
        fields(desc, ['code']); count();
        if (typeof desc.code !== 'string' || desc.code.length > 65536) fail('shader source limit');
        const obj = Object.freeze({}); shaders.set(obj, { device: d, code: desc.code }); return obj;
      },
      createComputePipeline(desc) {
        fields(desc, ['layout', 'compute']); fields(desc.compute, ['module', 'entryPoint']); count();
        if (desc.layout !== 'auto') fail('only auto pipeline layout is supported');
        const shader = owned(shaders, desc.compute.module, d);
        const entry = desc.compute.entryPoint ?? 'main';
        if (typeof entry !== 'string' || entry.length > 256) fail('invalid entry point');
        const p = { device: d, code: shader.code, entry };
        const layout = Object.freeze({}); layouts.set(layout, p);
        const obj = Object.freeze({ getBindGroupLayout(index) { live(d); if (index !== 0) fail('only group 0 supported'); return layout; } });
        pipelines.set(obj, p); return obj;
      },
      createBindGroup(desc) {
        fields(desc, ['layout', 'entries']); count();
        const pipeline = owned(layouts, desc.layout, d);
        if (!Array.isArray(desc.entries) || !desc.entries.length || desc.entries.length > 8) fail('1–8 bindings required');
        const entries = desc.entries.map((entry, i) => {
          fields(entry, ['binding', 'resource']); fields(entry.resource, ['buffer','offset','size']);
          if (entry.binding !== i) fail('bindings must be consecutive from 0');
          const b = buffer(entry.resource.buffer, d);
          if (!(b.usage & (U.STORAGE | U.UNIFORM))) fail('STORAGE or UNIFORM usage required');
          const offset=integer(entry.resource.offset??0,b.size,'binding offset');
          const size=integer(entry.resource.size??b.size-offset,b.size,'binding size');
          if(offset%256||!size||size%4||offset+size>b.size)fail('binding range/alignment');
          return {buffer:entry.resource.buffer,offset,size};
        });
        if (new Set(entries.map(e=>e.buffer)).size !== entries.length) fail('aliased bindings unsupported');
        const obj = Object.freeze({}); groups.set(obj, { device: d, pipeline, entries }); return obj;
      },
      createTexture(desc) {
        fields(desc, ['size','format','usage']); count();
        if (desc.format !== 'rgba8unorm' || desc.usage !== 17) fail('only rgba8unorm RENDER_ATTACHMENT | COPY_SRC textures supported');
        const size = desc.size;
        const width = integer(Array.isArray(size) ? size[0] : size?.width,1024,'width');
        const height = integer(Array.isArray(size) ? size[1] : size?.height,1024,'height');
        const depth = Array.isArray(size) ? (size[2] ?? 1) : (size?.depthOrArrayLayers ?? 1);
        if (!Array.isArray(size)) fields(size,['width','height','depthOrArrayLayers']);
        if (!width || !height || depth !== 1 || (Array.isArray(size) && size.length > 3)) fail('only 2D textures supported');
        const bytes = width*height*4;
        if (d.bytes+bytes>4*1024*1024) fail('cumulative resource limit (4 MiB)'); d.bytes+=bytes;
        const t = {device:d,width,height,dead:false};
        const obj = Object.freeze({width,height,format:'rgba8unorm',usage:17,
          createView(desc={}) { fields(desc,[]); count(); if(t.dead) fail('texture destroyed'); const v=Object.freeze({}); views.set(v,t); return v; },
          destroy() {t.dead=true;}
        }); textures.set(obj,t); return obj;
      },
      createRenderPipeline(desc) {
        fields(desc,['layout','vertex','fragment','primitive']); count();
        fields(desc.vertex,['module','entryPoint','buffers']); fields(desc.fragment,['module','entryPoint','targets']);
        if(desc.layout!=='auto') fail('only auto layout supported');
        fields(desc.primitive ?? {},['topology']);
        if((desc.primitive?.topology ?? 'triangle-list')!=='triangle-list') fail('only triangle-list supported');
        if(!Array.isArray(desc.fragment.targets)||desc.fragment.targets.length!==1) fail('one RGBA target required');
        fields(desc.fragment.targets[0],['format']);
        if(desc.fragment.targets[0].format!=='rgba8unorm') fail('only rgba8unorm supported');
        const vertex=owned(shaders,desc.vertex.module,d).code, fragment=owned(shaders,desc.fragment.module,d).code;
        const vl=desc.vertex.buffers ?? []; if(!Array.isArray(vl)||vl.length>4)fail('vertex buffer limit');
        const ls=vl.map(l=>{
          fields(l,['arrayStride','stepMode','attributes']);
          const stride=integer(l.arrayStride,2048,'vertex stride');
          if(!stride||stride%4||!['vertex','instance'].includes(l.stepMode??'vertex'))fail('invalid vertex layout');
          if(!Array.isArray(l.attributes)||l.attributes.length>8)fail('vertex attribute limit');
          return {stride,instance:l.stepMode==='instance',attributes:l.attributes.map(a=>{
            fields(a,['format','offset','shaderLocation']);if(!['float32x2','float32x3','float32x4'].includes(a.format))fail('unsupported vertex format');
            return {format:a.format,offset:integer(a.offset,2048,'attribute offset'),location:integer(a.shaderLocation,15,'shader location')};
          })};
        });
        const ve=desc.vertex.entryPoint??'main',fe=desc.fragment.entryPoint??'main';
        if(typeof ve!=='string'||typeof fe!=='string'||ve.length>256||fe.length>256)fail('invalid render entry point');
        const obj=Object.freeze({});renderPipelines.set(obj,{device:d,vertex,fragment,vertex_entry:ve,fragment_entry:fe,layouts:ls});return obj;
      },
      createCommandEncoder(desc = {}) {
        fields(desc, []); count();
        let begun = false, ended = false, finished = false, passOpen = false;
        const sequence = [];
        let shaderBytes = 0, dispatches = 0, copies = 0;
        let renderState;
        const editable = () => { live(d); if (finished) fail('encoder finished'); };
        return Object.freeze({
          beginComputePass(desc = {}) {
            fields(desc, []); editable();
            if (passOpen || renderState) fail('pass already active or mixed rendering unsupported');
            begun = true; passOpen = true;
            let passEnded = false, pipeline, group;
            const passLive = () => { editable(); if (passEnded) fail('pass ended'); };
            return Object.freeze({
              setPipeline(p) { passLive(); pipeline = owned(pipelines,p,d); },
              setBindGroup(index,g,offsets=[]) { passLive(); if(index!==0||offsets.length)fail('only group 0 without dynamic offsets'); group=owned(groups,g,d); },
              dispatchWorkgroups(x,y=1,z=1) {
                passLive();
                if(!pipeline||!group||group.pipeline!==pipeline)fail('incomplete or incompatible compute pass');
                const workgroups=[x,y,z].map(v=>integer(v,65536,'workgroup count'));
                if(workgroups.some(v=>!v)||x*y*z>65536)fail('workgroup limit');
                if(dispatches>=32||shaderBytes+pipeline.code.length>65536)fail('batch dispatch/shader limit');
                dispatches++;shaderBytes+=pipeline.code.length;
                sequence.push({kind:'compute',pipeline,group,workgroups});
              },
              end() { passLive(); passEnded=true;passOpen=false; }
            });
          },
          beginRenderPass(desc) {
            editable(); fields(desc,['colorAttachments']); if(begun||sequence.length)fail('mixed rendering unsupported'); begun=true;
            if(!Array.isArray(desc.colorAttachments)||desc.colorAttachments.length!==1)fail('one color attachment required');
            const a=desc.colorAttachments[0];fields(a,['view','clearValue','loadOp','storeOp']);
            if(a.loadOp!=='clear'||a.storeOp!=='store')fail('only clear/store supported');
            const texture=owned(views,a.view,d); if(texture.dead)fail('texture destroyed');
            const cv=a.clearValue??[0,0,0,0];
            if(!Array.isArray(cv))fields(cv,['r','g','b','a']);
            const clear=Array.isArray(cv)?cv.slice():[cv.r,cv.g,cv.b,cv.a];
            if(clear.length!==4||clear.some(x=>typeof x!=='number'||!Number.isFinite(x)))fail('invalid clear color');
            renderState={texture,clear,vertices:[],pipeline:null,draw:null,copy:null};
            const active=()=>{editable();if(ended||renderState.draw)fail('render pass ended or draw already encoded');};
            return Object.freeze({
              setPipeline(p){active();renderState.pipeline=owned(renderPipelines,p,d);},
              setVertexBuffer(slot,obj,offset=0,size){active();integer(slot,3,'vertex slot');const b=buffer(obj,d);if(!(b.usage&U.VERTEX)||offset!==0||(size!==undefined&&size!==b.size))fail('whole VERTEX buffer required');renderState.vertices[slot]=obj;},
              draw(vertices,instances=1,firstVertex=0,firstInstance=0){active();integer(vertices,65536,'vertex count');integer(instances,65536,'instance count');if(vertices*instances>65536||firstVertex!==0||firstInstance!==0)fail('draw limit');renderState.draw=[vertices,instances,0,0];},
              end(){editable();if(ended)fail('pass ended');ended=true;}
            });
          },
          copyTextureToBuffer(source,destination,size){
            editable();fields(source,['texture']);fields(destination,['buffer','bytesPerRow','rowsPerImage','offset']);
            if(!renderState||!ended||renderState.copy)fail('one copy after render pass required');
            const t=owned(textures,source.texture,d),b=buffer(destination.buffer,d);
            if(t!==renderState.texture||t.dead)fail('copy must use current color attachment');
            const w=Array.isArray(size)?size[0]:size?.width,h=Array.isArray(size)?size[1]:size?.height,z=Array.isArray(size)?size[2]??1:size?.depthOrArrayLayers??1;
            if(!Array.isArray(size))fields(size,['width','height','depthOrArrayLayers']);
            const pitch=integer(destination.bytesPerRow,4096,'bytesPerRow');
            if(w!==t.width||h!==t.height||z!==1||pitch%256||pitch<w*4||(destination.offset??0)!==0||(destination.rowsPerImage??h)!==h||b.size!==pitch*h||!(b.usage&U.COPY_DST))fail('only full padded texture readback supported');
            renderState.copy={buffer:destination.buffer,pitch};
          },
          copyBufferToBuffer(src, srcOffset, dst, dstOffset, size) {
            editable(); if(renderState||passOpen)fail('copy requires no active pass');
            if(copies>=8)fail('copy limit (8)');
            const a=buffer(src,d),b=buffer(dst,d);
            integer(srcOffset,a.size,'copy source offset');integer(dstOffset,b.size,'copy destination offset');integer(size,Math.min(a.size,b.size),'copy size');
            if(src===dst||!size||srcOffset%4||dstOffset%4||size%4||srcOffset+size>a.size||dstOffset+size>b.size)fail('copy bounds/alignment');
            if(!(a.usage&U.COPY_SRC)||!(b.usage&U.COPY_DST))fail('copy usage flags required');
            copies++;sequence.push({kind:'copy',source:src,destination:dst,source_offset:srcOffset,destination_offset:dstOffset,size});
          },
          finish(desc = {}) {
            fields(desc, []); editable();
            if(renderState){
              if(!ended||!renderState.draw||!renderState.pipeline||!renderState.copy)fail('incomplete render/readback command');
              if(renderState.pipeline.layouts.length!==renderState.vertices.length||renderState.vertices.some(v=>!v))fail('vertex buffers missing');
              finished=true;const obj=Object.freeze({});commands.set(obj,{device:d,render:renderState,used:false});return obj;
            }
            if(passOpen||!sequence.length)fail('incomplete command buffer');
            finished = true;
            const obj = Object.freeze({}); commands.set(obj, { device: d, sequence, units: Math.max(1,dispatches), used: false }); return obj;
          }
        });
      },
      queue: Object.freeze({
        writeBuffer(obj, offset, data, dataOffset = 0, size) {
          const b = buffer(obj, d);
          if (d.pending || b.mapped || b.mapping || !(b.usage & U.COPY_DST)) fail('buffer unavailable for upload');
          const view = ArrayBuffer.isView(data);
          if (!view && !(data instanceof ArrayBuffer)) throw new TypeError('BufferSource required');
          const unit = view ? (data.BYTES_PER_ELEMENT || 1) : 1;
          const source = new Uint8Array(view ? data.buffer : data, view ? data.byteOffset : 0, data.byteLength);
          integer(offset, b.size, 'offset'); integer(dataOffset, source.byteLength / unit, 'dataOffset');
          const bytes = size === undefined ? source.byteLength - dataOffset * unit : integer(size, source.byteLength / unit, 'size') * unit;
          if (offset % 4 || bytes % 4 || offset + bytes > b.size || dataOffset * unit + bytes > source.byteLength) fail('upload range/alignment');
          if (b.revision === 0xffffffff) fail('upload revision limit');
          b.revision++;
          b.data.set(source.subarray(dataOffset * unit, dataOffset * unit + bytes), offset);
        },
        submit(list) {
          live(d);
          if (d.pending || !Array.isArray(list) || list.length !== 1) fail('one command buffer and one submission in flight supported');
          const c = owned(commands, list[0], d); if (c.used) fail('command buffer already submitted');
          const units=c.render?1:c.units;
          if(d.submissions+units>32)fail('GPU work limit (32)');d.submissions+=units;
          if(c.render){
            const r=c.render;if(r.texture.dead)fail('texture destroyed');
            const bs=Array.from(r.vertices,obj=>buffer(obj,d)),out=buffer(r.copy.buffer,d);
            if([...bs,out].some(b=>b.mapped||b.mapping))fail('mapped render buffer');
            if(bs.reduce((n,b)=>n+b.size,0)>65536)fail('vertex byte limit (64 KiB)');
            const json=JSON.stringify({width:r.texture.width,height:r.texture.height,vertex:r.pipeline.vertex,fragment:r.pipeline.fragment,vertex_entry:r.pipeline.vertex_entry,fragment_entry:r.pipeline.fragment_entry,layouts:r.pipeline.layouts,draw:r.draw,clear:r.clear,usages:bs.map(b=>b.usage),refs:bs.map(b=>({id:b.id,revision:b.revision})),buffers:bs.map(b=>Array.from(b.data))});
            c.used=true;d.pending=true;
            d.tail=render(token,json).then(raw=>{
              live(d);
              if(out.dead||r.texture.dead)fail('render resource destroyed while pending');
              if(out.revision===0xffffffff)fail('copy revision limit');out.revision++;
              const packed=JSON.parse(raw);out.data.fill(0);
              for(let y=0;y<r.texture.height;y++)out.data.set(packed.slice(y*r.texture.width*4,(y+1)*r.texture.width*4),y*r.copy.pitch);
            }).finally(()=>{d.pending=false;});d.tail.catch(()=>{});return;
          }
          const targets=[],indices=new Map();
          function index(obj){
            const b=buffer(obj,d);if(b.mapped||b.mapping)fail('mapped buffer cannot be submitted');
            if(!indices.has(obj)){if(targets.length>=16)fail('batch resource limit (16)');indices.set(obj,targets.length);targets.push(b);}
            return indices.get(obj);
          }
          const encoded=c.sequence.map(step=>step.kind==='copy'
            ?{kind:'copy',source:index(step.source),destination:index(step.destination),source_offset:step.source_offset,destination_offset:step.destination_offset,size:step.size}
            :{kind:'compute',code:step.pipeline.code,entry:step.pipeline.entry,workgroups:step.workgroups,bindings:step.group.entries.map(b=>({buffer:index(b.buffer),offset:b.offset,size:b.size}))});
          const json=JSON.stringify({commands:encoded,usages:targets.map(b=>b.usage),refs:targets.map(b=>({id:b.id,revision:b.revision})),buffers:targets.map(b=>Array.from(b.data))});
          c.used=true;d.pending=true;
          d.tail=batch(token,json).then(raw=>{
            live(d);
            const result=JSON.parse(raw);
            let destroyed=false;
            for(let i=0;i<targets.length;i++){if(targets[i].dead)destroyed=true;else targets[i].data.set(result[i]);}
            if(destroyed)fail('buffer destroyed during submission');
          }).finally(()=>{d.pending=false;});
          // Retain the rejection for mapAsync/onSubmittedWorkDone, without an
          // unhandled rejection before callers attach their readback promise.
          d.tail.catch(() => {});
        },
        async onSubmittedWorkDone() { live(d); await d.tail; }
      })
    };
    Object.freeze(api);
    (async()=>{
      while(!d.dead){
        const pending=receiveEvents(token);unrefOp(pending);
        const delivery=JSON.parse(await pending);
        // A drained native event may have waited behind JS work or destroy().
        if(d.dead)return;eventCheck(token);
        if(delivery.closed)throw new Error('webgpu: event receiver closed');
        if(delivery.overflow)throw new Error('webgpu: native error queue overflow; session terminated');
        for(const record of delivery.errors){
          if(d.dead)return;eventCheck(token);
          const Type=record.kind==='validation'?GPUValidationError:record.kind==='out-of-memory'?GPUOutOfMemoryError:GPUInternalError;
          const error=new Type(record.message);
          if(handler){
            let prevented=false;
            const event=Object.freeze({type:'uncapturederror',target:api,error,cancelable:true,get defaultPrevented(){return prevented;},preventDefault(){prevented=true;}});
            handler.call(api,event);
          }
        }
        if(delivery.loss){
          if(d.dead)return;eventCheck(token);
          finish();return;
        }
      }
    })().catch(error=>{if(!d.dead)terminal(error);});
    return api;
  }
  Object.defineProperty(navigator, 'gpu', { value: Object.freeze({ requestAdapter }), configurable: true });
  globalThis.GPUValidationError=GPUValidationError;
  globalThis.GPUOutOfMemoryError=GPUOutOfMemoryError;
  globalThis.GPUInternalError=GPUInternalError;
  globalThis.GPUBufferUsage = U;
  globalThis.GPUTextureUsage = Object.freeze({COPY_SRC:1, RENDER_ATTACHMENT:16});
  globalThis.GPUMapMode = Object.freeze({ READ: 1 });
})();
