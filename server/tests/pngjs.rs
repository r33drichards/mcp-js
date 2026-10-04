//! Real pngjs@7.0.0 package coverage. The pinned esm.sh bundle is vendored
//! for an offline gate; the optional live test checks CDN graph drift.
use server::engine::module_loader::ModuleLoaderConfig;
use server::engine::{ExecutionConfig, execute_stateless, initialize_v8};
use std::collections::HashMap;
use std::sync::{Arc, Once};

static INIT: Once = Once::new();
const ROUNDTRIP: &str = include_str!("pngjs/roundtrip.js");

fn run(code: &str, config: &ModuleLoaderConfig) {
    INIT.call_once(initialize_v8);
    let (result, _) = execute_stateless(
        code,
        ExecutionConfig::new(64 * 1024 * 1024).module_loader_config(config),
    );
    assert!(result.is_ok(), "pngjs execution failed: {result:?}");
}

fn config() -> ModuleLoaderConfig {
    ModuleLoaderConfig {
        allow_external: true,
        policy_chain: None,
        virtual_modules: None,
        virtual_commonjs_modules: None,
        virtual_files: None,
    }
}

#[test]
fn pngjs_sync_and_async_pixel_roundtrips_offline() {
    let mut config = config();
    config.virtual_modules = Some(Arc::new(HashMap::from([(
        "https://esm.sh/pngjs@7.0.0".into(),
        include_str!("pngjs/pngjs-7.0.0.mjs").into(),
    )])));
    run(ROUNDTRIP, &config);
}

#[test]
#[ignore = "requires esm.sh; run cargo test --test pngjs -- --ignored"]
fn pngjs_sync_and_async_pixel_roundtrips_live() {
    run(ROUNDTRIP, &config());
}

#[test]
fn zlib_sync_streams_and_invalid_input() {
    run(
        r#"
        import {Buffer} from 'node:buffer';
        import zlib from 'node:zlib';
        const input = Buffer.from('synchronous and streaming compression');
        for (const [encode, decode] of [
            [zlib.deflateSync, zlib.inflateSync],
            [zlib.gzipSync, zlib.gunzipSync],
            [zlib.deflateRawSync, zlib.inflateRawSync],
        ]) {
            const compressed = encode(input, {level: 0});
            if (decode(compressed).toString() !== input.toString()) throw new Error('zlib roundtrip failed');
            let failed = false;
            try { decode(compressed.subarray(0, 2)); } catch { failed = true; }
            if (!failed) throw new Error('truncated compressed data was accepted');
        }
        for (const options of [{level: 10}, {level: 1.5}, {chunkSize: 1}]) {
            let failed = false;
            try { zlib.deflateSync(input, options); } catch { failed = true; }
            if (!failed) throw new Error('invalid zlib options were accepted');
        }
        const encoded = await new Promise((resolve, reject) => {
            const stream = zlib.createDeflate();
            const chunks = [];
            stream.on('data', chunk => chunks.push(chunk));
            stream.on('end', () => resolve(Buffer.concat(chunks)));
            stream.on('error', reject);
            stream.write(input.subarray(0, 5));
            stream.end(input.subarray(5));
        });
        if (zlib.inflateSync(encoded).toString() !== input.toString()) throw new Error('streaming write lost data');
    "#,
        &config(),
    );
}

#[test]
fn esm_sh_builtin_bridge_preserves_module_policy() {
    use server::engine::opa::{EvalMode, LocalPolicyEvaluator, PolicyChain, PolicyEvaluatorKind};
    let directory = tempfile::tempdir().unwrap();
    let policy = directory.path().join("modules.rego");
    std::fs::write(&policy, "package mcp.modules\nallow := false\n").unwrap();
    let evaluator =
        LocalPolicyEvaluator::from_file(&policy, "data.mcp.modules.allow".into()).unwrap();
    let mut config = config();
    config.policy_chain = Some(Arc::new(PolicyChain::new(
        vec![PolicyEvaluatorKind::Local(evaluator)],
        EvalMode::All,
    )));
    INIT.call_once(initialize_v8);
    let (result, _) = execute_stateless(
        "await import('https://esm.sh/node/stream.mjs');",
        ExecutionConfig::new(16 * 1024 * 1024).module_loader_config(&config),
    );
    let error = result.expect_err("CDN builtin aliases must not bypass module policy");
    assert!(error.contains("Module import denied by policy"), "{error}");
    config.allow_external = false;
    let (result, _) = execute_stateless(
        "await import('https://esm.sh/node/stream.mjs');",
        ExecutionConfig::new(16 * 1024 * 1024).module_loader_config(&config),
    );
    assert!(
        result
            .expect_err("external flag must still gate URLs")
            .contains("External module imports are disabled")
    );
}
