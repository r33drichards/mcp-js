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
