//! Rendering must use the same policy gate as compute, including each submission.
use server::engine::hooks::HookChain;
use server::engine::opa::{EvalMode, LocalPolicyEvaluator, PolicyChain, PolicyEvaluatorKind};
use server::engine::webgpu::GpuConfig;
use server::engine::{self, ExecutionConfig};
use std::sync::{Arc, Once};
static INIT: Once = Once::new();
#[test]
#[ignore = "real GPU required for allowed adapter initialization"]
fn render_policy_denies_before_shader_compilation() {
    INIT.call_once(engine::initialize_v8);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("policy.rego");
    std::fs::write(&path,"package mcp.gpu\ndefault allow = false\nallow if {input.operation == \"requestAdapter\"}\n").unwrap();
    let evaluator = LocalPolicyEvaluator::from_file(&path, "data.mcp.gpu.allow".into()).unwrap();
    let gpu = GpuConfig::new_with_hooks(Arc::new(HookChain::from_policy(
        "gpu",
        Arc::new(PolicyChain::new(
            vec![PolicyEvaluatorKind::Local(evaluator)],
            EvalMode::All,
        )),
    )));
    let script = format!(
        "const SHADER='deliberately invalid WGSL: policy must reject first';const CASE={{name:'denied'}};\n{}\n{}",
        include_str!("../../tests/webgpu/gallery/common.js"),
        include_str!("../../tests/webgpu/gallery/render.js")
    );
    let error = engine::execute_stateless(
        &script,
        ExecutionConfig::new(128 * 1024 * 1024).maybe_gpu_config(Some(&gpu)),
    )
    .0
    .unwrap_err();
    assert!(error.contains("policy denied"), "{error}");
}
