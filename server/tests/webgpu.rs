//! GPU tests opt in explicitly; ordinary CI still covers absence and policy denial.
use server::engine::hooks::HookChain;
use server::engine::opa::{EvalMode, LocalPolicyEvaluator, PolicyChain, PolicyEvaluatorKind};
use server::engine::webgpu::GpuConfig;
use server::engine::{self, ExecutionConfig};
use std::sync::{Arc, Once};
static INIT: Once = Once::new();
fn init() {
    INIT.call_once(engine::initialize_v8);
}
fn config(rego: &str) -> GpuConfig {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gpu.rego");
    std::fs::write(&path, rego).unwrap();
    let evaluator = LocalPolicyEvaluator::from_file(&path, "data.mcp.gpu.allow".into()).unwrap();
    GpuConfig::new_with_hooks(Arc::new(HookChain::from_policy(
        "gpu",
        Arc::new(PolicyChain::new(
            vec![PolicyEvaluatorKind::Local(evaluator)],
            EvalMode::All,
        )),
    )))
}
fn run(code: &str, gpu: Option<&GpuConfig>) -> Result<String, String> {
    init();
    engine::execute_stateless(
        code,
        ExecutionConfig::new(128 * 1024 * 1024).maybe_gpu_config(gpu),
    )
    .0
}
#[test]
fn disabled_by_default() {
    run(
        "if ('gpu' in navigator) throw new Error('ambient GPU access');",
        None,
    )
    .unwrap();
}
#[test]
fn denied_before_adapter_access() {
    let gpu = config("package mcp.gpu\ndefault allow = false\n");
    run("let denied = false; try { await navigator.gpu.requestAdapter(); } catch(e) { denied = String(e).includes('policy denied'); } if (!denied) throw new Error('not denied');", Some(&gpu)).unwrap();
}
#[test]
#[ignore = "requires a real GPU; run explicitly on a GPU host"]
fn upload_dispatch_readback_and_stale_snapshot() {
    init();
    let gpu = config("package mcp.gpu\ndefault allow = true\n");
    let (result, _) = engine::execute_stateful(
        include_str!("../../tests/webgpu/compute.js"),
        None,
        ExecutionConfig::new(128 * 1024 * 1024).maybe_gpu_config(Some(&gpu)),
    );
    let (_, wrapped, _) = result.unwrap();
    let payload = engine::unwrap_snapshot(&wrapped).unwrap();
    let code = r#"
      let stale = false;
      try { savedGpuDevice.queue.writeBuffer(savedGpuBuffer, 0, new Uint32Array([1])); }
      catch(e) { stale = String(e).includes('stale'); }
      if (!stale) throw new Error('snapshot handle reused');
      const adapter2 = await navigator.gpu.requestAdapter();
      if (!adapter2) throw new Error('GPU missing after restore');
      const device2 = await adapter2.requestDevice();
      // An old token must remain invalid even after a new session is created.
      try { savedGpuDevice.createBuffer({size:4,usage:128}); throw new Error('old token accepted'); }
      catch(e) { if (!String(e).includes('stale')) throw e; }
      device2.destroy();
    "#;
    engine::execute_stateful(
        code,
        Some(payload),
        ExecutionConfig::new(128 * 1024 * 1024).maybe_gpu_config(Some(&gpu)),
    )
    .0
    .unwrap();
}
#[test]
#[ignore = "requires a real GPU; run explicitly on a GPU host"]
fn invalid_wgsl_and_dispatch_policy() {
    let code = include_str!("../../tests/webgpu/compute.js");
    let deny_dispatch = config(
        "package mcp.gpu\ndefault allow = false\nallow if { input.operation == \"requestAdapter\" }\n",
    );
    let error = run(code, Some(&deny_dispatch)).unwrap_err();
    assert!(error.contains("policy denied"), "{error}");
    let allow = config("package mcp.gpu\ndefault allow = true\n");
    let invalid = code.replace("values[id.x] = values[id.x] * 2u;", "not valid WGSL!");
    let error = run(&invalid, Some(&allow)).unwrap_err();
    assert!(
        error.contains("webgpu") && !error.contains("unavailable"),
        "{error}"
    );
}

#[test]
#[ignore = "real GPU required"]
fn batch_policy_checks_all_commands_before_execution() {
    let gpu = config(
        "package mcp.gpu\ndefault allow = false\nallow if {input.operation == \"requestAdapter\"}\nallow if {input.operation == \"copyBuffer\"}\nallow if {input.operation == \"dispatch\"; input.workgroups[0] == 1}\n",
    );
    run(r#"
    const device=await (await navigator.gpu.requestAdapter()).requestDevice();
    const b=device.createBuffer({size:4,usage:140}),out=device.createBuffer({size:4,usage:9});
    const p=device.createComputePipeline({layout:'auto',compute:{module:device.createShaderModule({code:'@group(0) @binding(0) var<storage,read_write> a:array<u32>; @compute @workgroup_size(1) fn main(){a[0]+=1u;}'}),entryPoint:'main'}});
    const g=device.createBindGroup({layout:p.getBindGroupLayout(0),entries:[{binding:0,resource:{buffer:b}}]});
    const e=device.createCommandEncoder(),pass=e.beginComputePass();pass.setPipeline(p);pass.setBindGroup(0,g);pass.dispatchWorkgroups(1);pass.dispatchWorkgroups(2);pass.end();device.queue.submit([e.finish()]);let denied=false;try{await device.queue.onSubmittedWorkDone();}catch(e){denied=String(e).includes('policy denied');}if(!denied)throw Error('later dispatch policy not enforced');
    let dead=false;try{device.createCommandEncoder();}catch{dead=true;}if(!dead)throw Error('failed queue remained usable');device.destroy();
    "#, Some(&gpu)).unwrap();
    let deny_copy = config(
        "package mcp.gpu\ndefault allow = false\nallow if {input.operation == \"requestAdapter\"}\nallow if {input.operation == \"dispatch\"}\n",
    );
    let error = run(
        include_str!("../../tests/webgpu/compute.js"),
        Some(&deny_copy),
    )
    .unwrap_err();
    assert!(error.contains("policy denied"), "{error}");
}

#[test]
#[ignore = "real GPU required for allowed adapter initialization"]
fn partial_copy_policy_accounts_for_destination_allocation() {
    let gpu = config(
        "package mcp.gpu\ndefault allow = false\nallow if {input.operation == \"requestAdapter\"}\nallow if {input.operation == \"copyBuffer\"; input.bytes <= 1048576}\n",
    );
    run(r#"
    const d=await (await navigator.gpu.requestAdapter()).requestDevice();
    const source=d.createBuffer({size:4,usage:4}),destination=d.createBuffer({size:2*1024*1024,usage:8});
    const e=d.createCommandEncoder();e.copyBufferToBuffer(source,0,destination,0,4);d.queue.submit([e.finish()]);
    let denied=false;try{await d.queue.onSubmittedWorkDone();}catch(e){denied=String(e).includes('policy denied');}
    if(!denied)throw Error('small source bypassed destination allocation policy');d.destroy();
    "#,Some(&gpu)).unwrap();
}

#[test]
#[ignore = "real GPU required"]
fn fifo_policy_denial_cancels_suffix_and_terminates() {
    let gpu = config(
        "package mcp.gpu\ndefault allow = false\nallow if {input.operation == \"requestAdapter\"}\nallow if {input.operation == \"copyBuffer\"}\n",
    );
    run(r#"
 const d=await(await navigator.gpu.requestAdapter()).requestDevice(),a=d.createBuffer({size:4,usage:140}),out=d.createBuffer({size:4,usage:9});
 const copy=()=>{const e=d.createCommandEncoder();e.copyBufferToBuffer(a,0,out,0,4);return e.finish();};
 d.queue.submit([copy()]);const first=d.queue.onSubmittedWorkDone();
 const p=d.createComputePipeline({layout:'auto',compute:{module:d.createShaderModule({code:'@group(0) @binding(0) var<storage,read_write> a:u32; @compute @workgroup_size(1) fn main(){a=99u;}'})}}),g=d.createBindGroup({layout:p.getBindGroupLayout(0),entries:[{binding:0,resource:{buffer:a}}]});
 const e=d.createCommandEncoder(),pass=e.beginComputePass();pass.setPipeline(p);pass.setBindGroup(0,g);pass.dispatchWorkgroups(1);pass.end();d.queue.submit([e.finish()]);const middle=d.queue.onSubmittedWorkDone();middle.catch(()=>{});
 d.queue.submit([copy()]);const last=d.queue.onSubmittedWorkDone();last.catch(()=>{});
 const results=await Promise.allSettled([first,middle,last]);if(results.map(x=>x.status).join()!=='fulfilled,rejected,rejected')throw Error('queue denial order');
 if(!String(results[1].reason).includes('policy denied')||results[1].reason!==results[2].reason)throw Error('denial not propagated');
 let dead=false;try{d.createCommandEncoder();}catch{dead=true;}if(!dead)throw Error('failed session reusable');try{await d.lost;}catch{}d.destroy();
 "#,Some(&gpu)).unwrap();
}
