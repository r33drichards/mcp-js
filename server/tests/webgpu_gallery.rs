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
    INIT.call_once(engine::initialize_v8);
    let temp = tempfile::tempdir().unwrap();
    let db = sled::open(temp.path()).unwrap();
    let tree = db.open_tree("console").unwrap();
    let gpu = GpuConfig::new_with_hooks(Arc::new(HookChain::from_policy(
        "gpu",
        Arc::new(PolicyChain::new(vec![], EvalMode::All)),
    )));
    let code = format!(
        "const SHADER = {}; const CASE = {};\n{COMMON}\n{body}",
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
      await rejects(()=>device.createBuffer({size:16,usage:128,mappedAtCreation:true}),'mappedAtCreation');
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
