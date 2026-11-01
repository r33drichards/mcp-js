//! Pinned official compute references and separate analytical GPU rendering tests.
use server::engine::hooks::HookChain;
use server::engine::opa::{EvalMode, PolicyChain};
use server::engine::webgpu::GpuConfig;
use server::engine::{self, ExecutionConfig};
use std::sync::{Arc, Once};
static INIT: Once = Once::new();
const COMMON: &str = include_str!("../../tests/webgpu/gallery/common.js");
const BOIDS: &str = include_str!("../../tests/webgpu/gallery/upstream/boids.wgsl");
const LIFE: &str = include_str!("../../tests/webgpu/gallery/upstream/life.wgsl");
const BITONIC: &str = include_str!("../../tests/webgpu/gallery/upstream/bitonicCompute.ts");
fn run(shader: &str, case: serde_json::Value, body: &str) {
    run_with_common(shader, case, body, COMMON);
}
fn run_with_common(shader: &str, case: serde_json::Value, body: &str, common: &str) {
    INIT.call_once(engine::initialize_v8);
    let temp = tempfile::tempdir().unwrap();
    let db = sled::open(temp.path()).unwrap();
    let tree = db.open_tree("console").unwrap();
    let gpu = GpuConfig::new_with_hooks(Arc::new(HookChain::from_policy(
        "gpu",
        Arc::new(PolicyChain::new(vec![], EvalMode::All)),
    )));
    let code = format!(
        "const SHADER = {}; const CASE = {};\n{common}\n{body}",
        serde_json::to_string(shader).unwrap(),
        case
    );
    let result = engine::execute_stateless(
        &code,
        ExecutionConfig::new(128 * 1024 * 1024)
            .maybe_gpu_config(Some(&gpu))
            .console_tree(tree.clone()),
    )
    .0;
    result.unwrap_or_else(|e| panic!("case {case}: {e}"));
}
#[test]
#[ignore = "real GPU required"]
fn boids_compute_reference() {
    let shader = BOIDS.replace("var<uniform> params", "var<storage, read> params");
    assert_ne!(shader, BOIDS);
    for case in [
        serde_json::json!({"name":"cluster64","n":64,"steps":4}),
        serde_json::json!({"name":"cluster128","n":128,"steps":2}),
        serde_json::json!({"name":"wrap-speed-clamp","n":64,"steps":1,"boundary":true}),
    ] {
        run(
            &shader,
            case,
            include_str!("../../tests/webgpu/gallery/boids.js"),
        );
    }
}
#[test]
#[ignore = "real GPU required"]
fn life_unchanged_shader_reference() {
    for (name, w, h, steps) in [
        ("empty", 8, 8, 2),
        ("full", 8, 8, 2),
        ("block", 8, 8, 4),
        ("blinker", 16, 16, 4),
        ("wrap-glider", 16, 16, 4),
        ("seeded", 32, 16, 5),
    ] {
        run(
            LIFE,
            serde_json::json!({"name":name,"w":w,"h":h,"steps":steps}),
            include_str!("../../tests/webgpu/gallery/life.js"),
        );
    }
}
#[test]
#[ignore = "real GPU required"]
fn bitonic_local_global_and_atomics() {
    // Exact template expansion of upstream NaiveBitonicCompute(8), then one
    // uniform-to-storage binding change. No shader algorithm changes.
    let template = BITONIC
        .split_once("return `")
        .unwrap()
        .1
        .rsplit_once("`;")
        .unwrap()
        .0;
    let shader = template
        .replace("${workgroupSize * 2}", "16")
        .replace("${workgroupSize}", "8")
        .replace("var<uniform> uniforms", "var<storage, read> uniforms");
    assert!(!shader.contains("${"));
    for (name, n) in [
        ("duplicates", 16),
        ("reverse", 32),
        ("seeded", 64),
        ("equal", 16),
        ("sorted", 16),
    ] {
        run(
            &shader,
            serde_json::json!({"name":name,"n":n}),
            include_str!("../../tests/webgpu/gallery/bitonic.js"),
        );
    }
}
#[test]
#[ignore = "real GPU required"]
fn remaining_api_gaps_are_explicit() {
    run(
        "",
        serde_json::json!({"name":"api-gaps"}),
        r#"
      async function rejects(f,pattern){let error;try{await f();}catch(e){error=String(e);}assert(error && error.includes(pattern),'expected '+pattern+' got '+error);}
      await rejects(()=>navigator.gpu.requestAdapter({featureLevel:'compatibility'}),'unsupported option');
      await rejects(()=>device.createBuffer({size:16,usage:130,mappedAtCreation:true}),'MAP_WRITE only supports COPY_SRC');
      await rejects(()=>device.createComputePipeline({layout:'auto',compute:{module:device.createShaderModule({code:''}),constants:{blockSize:8}}}),'unsupported option');
      for(const method of ['createSampler','createBindGroupLayout','createPipelineLayout'])assert(typeof device[method]==='undefined',method+' unexpectedly present');
      assert(GPUBufferUsage.UNIFORM===64 && GPUBufferUsage.VERTEX===32,'usage constants incorrect');
      console.log('Confirmed remaining sampler/layout/adapter-option gaps');device.destroy();
    "#,
    );
}
#[test]
#[ignore = "real GPU required"]
fn offscreen_gpu_triangle_and_official_sprite() {
    let triangle = r#"
@vertex fn vs(@builtin(vertex_index) i:u32)->@builtin(position) vec4f {
  var positions=array<vec2f,3>(vec2f(-.75,-.75),vec2f(.75,-.75),vec2f(0.,.75));
  return vec4f(positions[i],0.,1.);
}
@fragment fn fs()->@location(0) vec4f {return vec4f(1.,0.,0.,1.);}
"#;
    run(
        triangle,
        serde_json::json!({"name":"triangle"}),
        include_str!("../../tests/webgpu/gallery/render.js"),
    );
    run(
        include_str!("../../tests/webgpu/gallery/upstream/sprite.wgsl"),
        serde_json::json!({"name":"sprite","sprite":true}),
        include_str!("../../tests/webgpu/gallery/render.js"),
    );
}
#[test]
#[ignore = "real GPU required"]
fn runtime_globals_for_onnx_assessment() {
    run(
        "",
        serde_json::json!({"name":"runtime-globals"}),
        r#"
      const names=['WebAssembly','SharedArrayBuffer','Atomics','Worker','fetch','Blob','URL','TextEncoder','ReadableStream','GPUValidationError'];
      const globals=Object.fromEntries(names.map(n=>[n,typeof globalThis[n]]));
      let wasm;try {await WebAssembly.instantiate(new Uint8Array([0,97,115,109,1,0,0,0]));wasm='minimal module instantiated';}catch(e){wasm=String(e);}
      console.log(JSON.stringify({globals,wasm,userAgent:navigator.userAgent,crossOriginIsolated:globalThis.crossOriginIsolated,adapterFeatures:typeof adapter.features,adapterLimits:typeof adapter.limits,deviceLost:typeof device.lost}));device.destroy();
    "#,
    );
}
#[test]
#[ignore = "real GPU required"]
fn boids_unchanged_uniform_shader_reference() {
    run(
        BOIDS,
        serde_json::json!({"name":"unchanged-uniform128","n":128,"steps":3,"uniform":true}),
        include_str!("../../tests/webgpu/gallery/boids.js"),
    );
}
#[test]
#[ignore = "real GPU required"]
fn boids_unchanged_compute_and_gpu_render() {
    let body = format!(
        "const SPRITE={};\n{}",
        serde_json::to_string(include_str!(
            "../../tests/webgpu/gallery/upstream/sprite.wgsl"
        ))
        .unwrap(),
        include_str!("../../tests/webgpu/gallery/boids-render.js")
    );
    run(
        BOIDS,
        serde_json::json!({"name":"boids-compute-render"}),
        &body,
    );
}

#[test]
#[ignore = "real GPU required"]
fn boids_resident_compute_to_render() {
    let sprite = include_str!("../../tests/webgpu/gallery/upstream/sprite.wgsl");
    let body = format!(
        "const SPRITE = {};\n{}",
        serde_json::to_string(sprite).unwrap(),
        include_str!("../../tests/webgpu/gallery/boids-resident-render.js")
    );
    run(BOIDS, serde_json::json!({"name":"resident-boids"}), &body);
}

#[test]
#[ignore = "real GPU required"]
fn render_copy_updates_resident_compute_buffer() {
    run(
        "",
        serde_json::json!({"name":"render-copy-revision"}),
        r#"
    const b=device.createBuffer({size:256,usage:140}),out=device.createBuffer({size:256,usage:9});
    const p=device.createComputePipeline({layout:'auto',compute:{module:device.createShaderModule({code:'@group(0) @binding(0) var<storage,read_write> a:array<u32>; @compute @workgroup_size(1) fn main(){a[0]+=1u;}'}),entryPoint:'main'}});
    const g=device.createBindGroup({layout:p.getBindGroupLayout(0),entries:[{binding:0,resource:{buffer:b}}]});
    async function compute(){const e=device.createCommandEncoder(),pass=e.beginComputePass();pass.setPipeline(p);pass.setBindGroup(0,g);pass.dispatchWorkgroups(1);pass.end();e.copyBufferToBuffer(b,0,out,0,256);device.queue.submit([e.finish()]);await device.queue.onSubmittedWorkDone();}
    await compute();
    const m=device.createShaderModule({code:'@vertex fn vs(@builtin(vertex_index) i:u32)->@builtin(position) vec4f{return vec4f(0,0,0,1);} @fragment fn fs()->@location(0) vec4f{return vec4f(1,1,1,1);}'});
    const rp=device.createRenderPipeline({layout:'auto',vertex:{module:m,entryPoint:'vs'},fragment:{module:m,entryPoint:'fs',targets:[{format:'rgba8unorm'}]}});
    const t=device.createTexture({size:[1,1],format:'rgba8unorm',usage:17});
    const e=device.createCommandEncoder(),pass=e.beginRenderPass({colorAttachments:[{view:t.createView(),clearValue:[0,0,0,1],loadOp:'clear',storeOp:'store'}]});pass.setPipeline(rp);pass.draw(0);pass.end();e.copyTextureToBuffer({texture:t},{buffer:b,bytesPerRow:256},[1,1]);device.queue.submit([e.finish()]);await device.queue.onSubmittedWorkDone();
    await compute();await out.mapAsync(1);assert(new Uint32Array(out.getMappedRange())[0]===0xff000001,'render readback copy did not invalidate resident revision');out.unmap();device.destroy();
    "#,
    );
}

#[test]
#[ignore = "real GPU required"]
fn ordered_native_copies_and_multiple_dispatches() {
    run(
        "",
        serde_json::json!({"name":"ordered-native-commands"}),
        include_str!("../../tests/webgpu/gallery/commands.js"),
    );
}
#[test]
#[ignore = "real GPU required"]
fn batch_charges_every_dispatch_to_execution_budget() {
    run(
        "",
        serde_json::json!({"name":"batch-work-budget"}),
        r#"
    const b=device.createBuffer({size:4,usage:140}),out=device.createBuffer({size:4,usage:9});
    const p=device.createComputePipeline({layout:'auto',compute:{module:device.createShaderModule({code:'@group(0) @binding(0) var<storage,read_write> a:array<u32>; @compute @workgroup_size(1) fn main(){a[0]+=1u;}'}),entryPoint:'main'}});
    const g=device.createBindGroup({layout:p.getBindGroupLayout(0),entries:[{binding:0,resource:{buffer:b}}]});
    const e=device.createCommandEncoder(),pass=e.beginComputePass();pass.setPipeline(p);pass.setBindGroup(0,g);for(let i=0;i<32;i++)pass.dispatchWorkgroups(1);pass.end();e.copyBufferToBuffer(b,0,out,0,4);device.queue.submit([e.finish()]);await out.mapAsync(1);assert(new Uint32Array(out.getMappedRange())[0]===32,'dependent dispatch result');out.unmap();
    const e2=device.createCommandEncoder();e2.copyBufferToBuffer(b,0,out,0,4);let rejected=false;try{device.queue.submit([e2.finish()]);}catch(e){rejected=String(e).includes('work limit');}assert(rejected,'batch bypassed work budget');device.destroy();
    "#,
    );
}

#[test]
#[ignore = "real GPU required for adapter capability probe"]
fn onnx_simd_and_first_bootstrap_blocker() {
    // SIMD probe bytes from microsoft/onnxruntime at
    // 2927f3b3fcd2012394c8a4bb8d2d18f81cc0f768,
    // js/web/lib/wasm/wasm-factory.ts; MIT license in upstream/ONNX-LICENSE.
    run(
        "",
        serde_json::json!({"name":"onnx-bootstrap-prerequisites"}),
        r#"
    const simd=new Uint8Array([0,97,115,109,1,0,0,0,1,4,1,96,0,0,3,2,1,0,10,30,1,28,0,65,0,253,15,253,12,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,253,186,1,26,11]);
    assert(WebAssembly.validate(simd),'ONNX SIMD capability probe failed');await WebAssembly.instantiate(simd);
    assert(adapter.limits.maxComputeWorkgroupStorageSize>0&&adapter.features.size===0,'native capability profile missing');
    assert(Reflect.set(device,'onuncapturederror',()=>{}),'native event handler unavailable');
    assert(typeof device.lost.then==='function','loss promise unavailable');device.destroy();assert((await device.lost).reason==='destroyed','explicit destruction loss');
    "#,
    );
}

#[test]
#[ignore = "real GPU required"]
fn pooled_binding_ranges_and_partial_copy_sentinels() {
    run(
        "",
        serde_json::json!({"name":"pooled-ranges"}),
        include_str!("../../tests/webgpu/gallery/ranges.js"),
    );
}

#[test]
#[ignore = "real GPU required"]
fn uniform_vertex_usage_is_stable_across_all_first_use_paths() {
    for order in [
        "compute-render",
        "render-compute",
        "copy-compute-render",
        "copy-render-compute",
    ] {
        run(
            "",
            serde_json::json!({"order":order}),
            include_str!("../../tests/webgpu/gallery/union-usage.js"),
        );
    }
}

#[test]
#[ignore = "real GPU required"]
fn native_capability_profile_and_request_validation() {
    run_with_common(
        "",
        serde_json::json!({}),
        r#"
 const a=await navigator.gpu.requestAdapter({powerPreference:'high-performance',forceFallbackAdapter:false});if(!a)throw Error('GPU missing');
 const assert=(x,m)=>{if(!x)throw Error(m);};
 const rejects=async f=>{let failed=false;try{await f();}catch{failed=true;}assert(failed,'invalid profile accepted');};
 assert(a.limits.maxBufferSize>0&&a.limits.maxComputeInvocationsPerWorkgroup>0,'missing native limits');
 assert(a.features.size===0&&!a.features.has('shader-f16')&&[...a.features].length===0,'unbacked optional feature');
 assert(typeof a.info.vendor==='string'&&typeof a.info.description==='string'&&(await a.requestAdapterInfo())===a.info,'native adapter info missing');
 assert(Object.isFrozen(a.limits)&&Object.isFrozen(a.info)&&typeof a.features.add==='undefined','mutable capability surface');
 await rejects(()=>a.requestDevice({requiredFeatures:['shader-f16']}));
 await rejects(()=>a.requestDevice({requiredLimits:{maxBufferSize:a.limits.maxBufferSize+4}}));
 await rejects(()=>a.requestDevice({requiredLimits:{maxBufferSize:4}}));
 await rejects(()=>a.requestDevice({requiredLimits:{unknown:0}}));
 const d=await a.requestDevice({requiredFeatures:[],requiredLimits:a.limits});
 assert(d.limits.maxBufferSize===a.limits.maxBufferSize&&d.features.size===0,'native device profile changed');
 let quota=false;try{d.createBuffer({size:4194308,usage:128});}catch{quota=true;}assert(quota,'native limits weakened runtime allocation budget');
 d.destroy();await rejects(()=>a.requestAdapterInfo());
 "#,
        "",
    );
}

#[test]
#[ignore = "real GPU required"]
fn effective_limits_compute_binding_boundary() {
    run_with_common(
        "",
        serde_json::json!({}),
        r#"
 const a=await navigator.gpu.requestAdapter();const d=await a.requestDevice({requiredLimits:a.limits});
 const l=d.limits;const assert=(x,m)=>{if(!x)throw Error(m);};
 assert(l.maxBindGroups===1&&l.maxBindingsPerBindGroup===8,'binding subset overstated');
 for(const name of ['maxSampledTexturesPerShaderStage','maxSamplersPerShaderStage','maxStorageTexturesPerShaderStage','maxDynamicUniformBuffersPerPipelineLayout','maxDynamicStorageBuffersPerPipelineLayout','maxTextureDimension1D','maxTextureDimension3D'])assert(l[name]===0,`${name} unsupported`);
 const n=l.maxStorageBuffersPerShaderStage;
 const buffers=Array.from({length:n},()=>d.createBuffer({size:4,usage:140}));
 const code=buffers.map((_,i)=>`@group(0) @binding(${i}) var<storage,read_write> b${i}:u32;`).join('\n')+`@compute @workgroup_size(1) fn main(){${buffers.map((_,i)=>`b${i}=${i+11}u;`).join('')}}`;
 const p=d.createComputePipeline({layout:'auto',compute:{module:d.createShaderModule({code})}});
 const g=d.createBindGroup({layout:p.getBindGroupLayout(l.maxBindGroups-1),entries:buffers.map((buffer,binding)=>({binding,resource:{buffer}}))});
 let failed=false;try{p.getBindGroupLayout(l.maxBindGroups);}catch{failed=true;}assert(failed,'out-of-profile group accepted');
 const e=d.createCommandEncoder(),pass=e.beginComputePass();pass.setPipeline(p);pass.setBindGroup(0,g);pass.dispatchWorkgroups(1);pass.end();
 const out=d.createBuffer({size:4,usage:9});e.copyBufferToBuffer(buffers[n-1],0,out,0,4);d.queue.submit([e.finish()]);
 await out.mapAsync(1);assert(new Uint32Array(out.getMappedRange())[0]===n+10,'advertised storage binding boundary failed');out.unmap();d.destroy();
 "#,
        "",
    );
}

#[test]
#[ignore = "real GPU required"]
fn effective_limits_allocation_boundaries() {
    // Separate executions keep cumulative allocations distinct from per-object maxima.
    run_with_common(
        "",
        serde_json::json!({}),
        r#"
 const a=await navigator.gpu.requestAdapter(),d=await a.requestDevice({requiredLimits:a.limits});
 if(d.limits.maxBufferSize!==4194304||d.limits.maxStorageBufferBindingSize!==4194304)throw Error('buffer cap');
 const b=d.createBuffer({size:d.limits.maxBufferSize,usage:128});
 const p=d.createComputePipeline({layout:'auto',compute:{module:d.createShaderModule({code:'@group(0) @binding(0) var<storage,read_write> b:array<u32>; @compute @workgroup_size(1) fn main(){b[0]=17u;}'})}});
 const g=d.createBindGroup({layout:p.getBindGroupLayout(0),entries:[{binding:0,resource:{buffer:b,size:d.limits.maxStorageBufferBindingSize}}]});
 const e=d.createCommandEncoder(),pass=e.beginComputePass();pass.setPipeline(p);pass.setBindGroup(0,g);pass.dispatchWorkgroups(1);pass.end();d.queue.submit([e.finish()]);await d.queue.onSubmittedWorkDone();d.destroy();
 "#,
        " ",
    );
    run_with_common(
        "",
        serde_json::json!({}),
        r#"
 const a=await navigator.gpu.requestAdapter(),d=await a.requestDevice({requiredLimits:a.limits});
 if(d.limits.maxTextureDimension2D!==1024||d.limits.maxTextureArrayLayers!==1||d.limits.maxColorAttachments!==1||d.limits.maxColorAttachmentBytesPerSample!==4)throw Error('texture cap');
 const t=d.createTexture({size:[d.limits.maxTextureDimension2D,1,d.limits.maxTextureArrayLayers],format:'rgba8unorm',usage:17});
 const module=d.createShaderModule({code:'@vertex fn vs(@builtin(vertex_index) i:u32)->@builtin(position) vec4f{var p=array<vec2f,3>(vec2f(-1,-1),vec2f(3,-1),vec2f(-1,3));return vec4f(p[i],0,1);} @fragment fn fs()->@location(0) vec4f{return vec4f(1,0,0,1);}'});
 const p=d.createRenderPipeline({layout:'auto',vertex:{module,entryPoint:'vs'},fragment:{module,entryPoint:'fs',targets:[{format:'rgba8unorm'}]}});
 const e=d.createCommandEncoder(),pass=e.beginRenderPass({colorAttachments:[{view:t.createView(),loadOp:'clear',storeOp:'store'}]});pass.setPipeline(p);pass.draw(3);pass.end();
 const out=d.createBuffer({size:4096,usage:9});e.copyTextureToBuffer({texture:t},{buffer:out,bytesPerRow:4096},[1024,1]);d.queue.submit([e.finish()]);await out.mapAsync(1);const pixels=new Uint8Array(out.getMappedRange());if(pixels[0]!==255||pixels[4092]!==255||pixels[4095]!==255)throw Error('advertised texture width failed rendering');out.unmap();
 let failed=false;try{d.createTexture({size:[d.limits.maxTextureDimension2D+1,1],format:'rgba8unorm',usage:17});}catch{failed=true;}if(!failed)throw Error('oversized texture accepted');d.destroy();
 "#,
        " ",
    );
}

#[test]
#[ignore = "real GPU required"]
fn event_receiver_does_not_keep_execution_alive() {
    run_with_common(
        "",
        serde_json::json!({}),
        r#"
 const d=await(await navigator.gpu.requestAdapter()).requestDevice();
 d.onuncapturederror=()=>{throw Error('unexpected event');};
 // No destroy: teardown must close a pending unreferenced receiver and GPU session.
 "#,
        "",
    );
}

#[test]
#[ignore = "real GPU required"]
fn mapped_initialization_upload_compute_and_detach() {
    run_with_common(
        "",
        serde_json::json!({}),
        r#"
 const d=await(await navigator.gpu.requestAdapter()).requestDevice();
 const upload=d.createBuffer({size:16,usage:GPUBufferUsage.MAP_WRITE|GPUBufferUsage.COPY_SRC,mappedAtCreation:true});
 const initial=upload.getMappedRange();if(new Uint32Array(initial).some(x=>x))throw Error('initial bytes not zero');new Uint32Array(initial).set([11,22,33]);upload.unmap();if(initial.byteLength)throw Error('initial range not detached');
 const b=d.createBuffer({size:16,usage:140}),out=d.createBuffer({size:16,usage:9});
 const p=d.createComputePipeline({layout:'auto',compute:{module:d.createShaderModule({code:'@group(0) @binding(0) var<storage,read_write> b:array<u32>; @compute @workgroup_size(1) fn main(@builtin(global_invocation_id) id:vec3u){b[id.x]+=1u;}'})}});
 const g=d.createBindGroup({layout:p.getBindGroupLayout(0),entries:[{binding:0,resource:{buffer:b}}]});
 const e=d.createCommandEncoder();e.copyBufferToBuffer(upload,0,b,0,16);const pass=e.beginComputePass();pass.setPipeline(p);pass.setBindGroup(0,g);pass.dispatchWorkgroups(4);pass.end();e.copyBufferToBuffer(b,0,out,0,16);d.queue.submit([e.finish()]);await out.mapAsync(1);
 const range=out.getMappedRange();if(Array.from(new Uint32Array(range)).join()!=='12,23,34,1')throw Error('native initialization/copy/compute mismatch');
 d.destroy();if(range.byteLength)throw Error('readback range survives device destruction');
 "#,
        "",
    );
}

#[test]
#[ignore = "real GPU required"]
fn mapped_shadowed_length_teardown() {
    for shadow in ["throw", "zero"] {
        for action in ["unmap", "buffer", "device"] {
            run_with_common(
                "",
                serde_json::json!({"shadow":shadow,"action":action}),
                r#"
 const adapter=await navigator.gpu.requestAdapter(),d=await adapter.requestDevice();
 const b=d.createBuffer({size:16,usage:6,mappedAtCreation:true}),range=b.getMappedRange();
 const length=Object.getOwnPropertyDescriptor(ArrayBuffer.prototype,'byteLength').get;
 Object.defineProperty(range,'byteLength',CASE.shadow==='throw'?{get(){throw Error('caller getter');}}:{value:0});
 if(CASE.action==='unmap')b.unmap();else if(CASE.action==='buffer')b.destroy();else d.destroy();
 if(length.call(range)!==0||b.mapState!=='unmapped')throw Error('range remains mapped');
 d.destroy();d.destroy();if((await d.lost).reason!=='destroyed')throw Error('loss not settled');
 let stale=false;try{await adapter.requestAdapterInfo();}catch{stale=true;}if(!stale)throw Error('native session survived destroy');
 "#,
                "",
            );
        }
    }
}
