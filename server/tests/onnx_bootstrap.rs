//! Explicit opt-in probe of pinned official assets. No external module access.
use base64::Engine;
use server::engine::{
    self, ExecutionConfig,
    hooks::HookChain,
    module_loader::ModuleLoaderConfig,
    opa::{EvalMode, LocalPolicyEvaluator, PolicyChain, PolicyEvaluatorKind},
    webgpu::GpuConfig,
};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, path::PathBuf, sync::Arc};
#[test]
#[ignore = "requires verified pinned assets via MCP_ONNX_ASSET_DIR and real GPU"]
fn pinned_add_smoke() {
    run_pinned_add(false);
}
#[test]
#[ignore = "requires verified pinned assets via MCP_ONNX_ASSET_DIR and real GPU"]
fn pinned_add_denied_dispatch_does_not_fallback() {
    run_pinned_add(true);
}
fn run_pinned_add(deny_dispatch: bool) {
    engine::initialize_v8();
    let root =
        PathBuf::from(std::env::var("MCP_ONNX_ASSET_DIR").expect("set verified asset directory"));
    let files = [
        (
            "ort.all.min.mjs",
            "2478830b23b6d9c7aa6b1031700c17820850ef1864048037178478f229d50e11",
        ),
        (
            "ort-wasm-simd-threaded.jsep.mjs",
            "c7b70c8003f077c62849e15275d0b1f1d1cf1a6dfc63b967400567c7224bfdbe",
        ),
        (
            "ort-wasm-simd-threaded.jsep.wasm",
            "ac0d8f1cfbf2732420ad78a86072b8507253c63885a61c3fd640b3055256d31d",
        ),
    ];
    let mut modules = HashMap::new();
    let mut wasm = Vec::new();
    for (name, hash) in files {
        let bytes = std::fs::read(root.join(name)).unwrap();
        assert_eq!(format!("{:x}", Sha256::digest(&bytes)), hash);
        if name.ends_with(".wasm") {
            wasm = bytes;
        } else {
            modules.insert(
                format!("file:///onnx/{name}"),
                String::from_utf8(bytes).unwrap(),
            );
        }
    }
    let loader = ModuleLoaderConfig {
        allow_external: false,
        hooks: None,
        virtual_modules: Some(Arc::new(modules)),
        virtual_commonjs_modules: None,
        virtual_files: None,
    };
    let policy_dir = tempfile::tempdir().unwrap();
    let policy_path = policy_dir.path().join("gpu.rego");
    std::fs::write(
        &policy_path,
        if deny_dispatch {
            "package mcp.gpu\ndefault allow = false\nallow if { input.operation != \"dispatch\" }\n"
        } else {
            "package mcp.gpu\ndefault allow = true\n"
        },
    )
    .unwrap();
    let policy =
        LocalPolicyEvaluator::from_file(&policy_path, "data.mcp.gpu.allow".into()).unwrap();
    let gpu = GpuConfig::new_with_hooks(Arc::new(HookChain::from_policy(
        "gpu",
        Arc::new(PolicyChain::new(
            vec![PolicyEvaluatorKind::Local(policy)],
            EvalMode::All,
        )),
    )));
    let chunks: Vec<_> = wasm
        .chunks(48 * 1024)
        .map(|chunk| base64::engine::general_purpose::STANDARD.encode(chunk))
        .collect();
    let chunks_json = serde_json::to_string(&chunks).unwrap();
    let bootstrap = format!(
        "(()=>{{const out=new Uint8Array({});let offset=0;for(const chunk of {}){{const binary=atob(chunk);for(let i=0;i<binary.length;i++)out[offset++]=binary.charCodeAt(i);}}globalThis.__onnxProbeWasmBytes=out;}})();",
        wasm.len(),
        chunks_json
    );
    let model =
        serde_json::to_string(include_bytes!("../../tests/webgpu/onnx_add4.onnx").as_slice())
            .unwrap();
    let code = format!(
        r#"
    const ort=await import('file:///onnx/ort.all.min.mjs');
    if(ort.env.versions.web!=='1.23.2')throw Error('version mismatch');
    ort.env.wasm.numThreads=1;ort.env.wasm.proxy=false;ort.env.wasm.initTimeout=15000;
    ort.env.wasm.wasmPaths={{mjs:'file:///onnx/ort-wasm-simd-threaded.jsep.mjs'}};
    ort.env.wasm.wasmBinary=globalThis.__onnxProbeWasmBytes;delete globalThis.__onnxProbeWasmBytes;
    const session=await ort.InferenceSession.create(new Uint8Array({model}),{{executionProviders:['webgpu'],graphOptimizationLevel:'disabled',extra:{{session:{{disable_cpu_ep_fallback:'1'}}}},logSeverityLevel:0,logVerbosityLevel:1}});
    console.log('ONNX_PROBE_SESSION_CREATED');
    const result=await session.run({{A:new ort.Tensor('float32',new Float32Array([1,2,3,4]),[4]),B:new ort.Tensor('float32',new Float32Array([10,20,30,40]),[4])}});
    if(JSON.stringify(Array.from(result.Y.data))!=='[11,22,33,44]')throw Error('Add output mismatch');
    await session.release();
    console.log('ONNX_ADD_OUTPUT_VERIFIED [11,22,33,44]; CPU_EP_FALLBACK_DISABLED');
    "#
    );
    let temp = tempfile::tempdir().unwrap();
    let db = sled::open(temp.path()).unwrap();
    let console = db.open_tree("console").unwrap();
    let stderr = db.open_tree("stderr").unwrap();
    let result = engine::execute_stateless(
        &code,
        ExecutionConfig::new(128 * 1024 * 1024)
            .module_loader_config(&loader)
            .bootstrap_script(&bootstrap)
            .console_tree(console.clone())
            .console_error_tree(stderr.clone())
            .maybe_gpu_config(Some(&gpu)),
    )
    .0;
    for tree in [&console, &stderr] {
        for (_, value) in tree.iter().flatten() {
            eprintln!("asset console: {:.2000}", String::from_utf8_lossy(&value));
        }
    }
    let diagnostics: Vec<String> = stderr
        .iter()
        .flatten()
        .map(|(_, value)| String::from_utf8_lossy(&value).into_owned())
        .collect();
    assert!(
        diagnostics
            .iter()
            .any(|line| line.contains("session.disable_cpu_ep_fallback: 1")),
        "runtime did not confirm fallback-disabled configuration"
    );
    for diagnostic in &diagnostics {
        for line in diagnostic
            .lines()
            .filter(|line| line.contains("placed") || line.contains("VerifyEachNode"))
        {
            eprintln!("ONNX_PROVIDER_PLACEMENT: {line}");
        }
    }
    let logs: Vec<String> = console
        .iter()
        .flatten()
        .map(|(_, value)| String::from_utf8_lossy(&value).into_owned())
        .collect();
    assert!(
        logs.iter()
            .any(|line| line.contains("ONNX_PROBE_SESSION_CREATED")),
        "session was not created"
    );
    if deny_dispatch {
        let error = result.expect_err("GPU-denied Add unexpectedly succeeded");
        assert!(
            error.contains("policy denied"),
            "unexpected denial: {error}"
        );
        assert!(
            !logs
                .iter()
                .any(|line| line.contains("ONNX_ADD_OUTPUT_VERIFIED"))
        );
        eprintln!("ONNX_GPU_DISPATCH_DENIAL_VERIFIED: {error:.1200}");
    } else {
        result.unwrap_or_else(|error| panic!("Pinned bootstrap probe: {error:.2500}"));
        assert!(
            logs.iter()
                .any(|line| line.contains("ONNX_ADD_OUTPUT_VERIFIED"))
        );
    }
}
