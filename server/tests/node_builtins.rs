//! Integration tests for the node: builtins that have no vendorable
//! upstream coverage: module/createRequire, timers + timers/promises,
//! https and fs/promises stubs, stream/web re-exports, and the registry ↔
//! builtinModules sync. (timers, console, and crypto conformance runs in
//! the vendored Node core suite, tests/node_compat.rs.)

use std::sync::{Arc, Once};

use server::engine::execution::ExecutionRegistry;
use server::engine::{Engine, initialize_v8};

static INIT: Once = Once::new();

fn ensure_v8() {
    INIT.call_once(|| {
        initialize_v8();
    });
}

fn create_test_engine() -> Engine {
    let tmp = std::env::temp_dir().join(format!(
        "mcp-node-builtins-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let registry =
        ExecutionRegistry::new(tmp.to_str().unwrap()).expect("Failed to create test registry");
    Engine::new_stateless(64 * 1024 * 1024, 30, 4).with_execution_registry(Arc::new(registry))
}

async fn run_js(engine: &Engine, code: &str) -> Result<String, String> {
    let exec_id = engine
        .run_js(code.to_string())
        .execute()
        .await
        .map_err(|error| format!("submit should succeed: {error}"))?;

    for _ in 0..600 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        if let Ok(info) = engine.get_execution(&exec_id) {
            match info.status.as_str() {
                "completed" => return Ok(info.result.unwrap_or_default()),
                "failed" => return Err(info.error.unwrap_or_default()),
                "timed_out" => return Err("execution timed out".to_string()),
                _ => continue,
            }
        }
    }

    Err("timeout waiting for execution".to_string())
}

async fn expect_ok(code: &str) {
    ensure_v8();
    let engine = create_test_engine();
    if let Err(error) = run_js(&engine, code).await {
        panic!("execution failed: {error}");
    }
}

#[test]
fn node_compat_prelude_maps_registered_host_modules() {
    let prelude = include_str!("node_compat/runner/prelude.js");
    for (import, mapping) in [
        ("node:dns", "dns: dns"),
        ("node:fs", "fs: fs"),
        ("node:fs/promises", "'fs/promises': fsPromises"),
        ("node:http", "http: http"),
        ("node:http2", "http2: http2"),
        ("node:https", "https: https"),
        ("node:net", "net: net"),
        ("node:stream/web", "'stream/web': streamWeb"),
        ("node:tls", "tls: tls"),
        ("node:zlib", "zlib: zlib"),
    ] {
        assert!(
            prelude.contains(import),
            "prelude missing import for {import}"
        );
        assert!(
            prelude.contains(mapping),
            "prelude missing require mapping {mapping}"
        );
    }
}

#[tokio::test]
async fn node_compat_prelude_wraps_commonjs_body() {
    let prelude = include_str!("node_compat/runner/prelude.js");
    expect_ok(&format!(
        r#"
        {prelude}
        globalThis.__NODE_TEST_RUN_CJS__(`
            if (this !== module.exports) throw new Error('wrong CommonJS this');
            if (arguments[0] !== exports || arguments[1] !== require ||
                arguments[2] !== module || arguments[3] !== __filename ||
                arguments[4] !== __dirname) {{
                throw new Error('wrong CommonJS argument order');
            }}
            return;
            throw new Error('top-level return did not exit the wrapper');
        `);
        "#
    ))
    .await;
}

#[tokio::test]
async fn node_compat_prelude_skips_when_inspector_disabled() {
    let prelude = include_str!("node_compat/runner/prelude.js");
    expect_ok(&format!(
        r#"
        {prelude}
        const commonHarness = require('../common');
        if (commonHarness.hasInspector !== false) throw new Error('inspector must be disabled');
        try {{
            commonHarness.skipIfInspectorDisabled();
            throw new Error('expected inspector skip');
        }} catch (error) {{
            if (!error.__nodeTestSkip) throw error;
        }}
        const report = JSON.parse(globalThis.__NODE_TEST_REPORT__());
        if (report.skipped !== 'V8 inspector is disabled') {{
            throw new Error('wrong inspector skip reason: ' + report.skipped);
        }}
        "#
    ))
    .await;
}

/// module.builtinModules must list exactly the registry, so the eagerly
/// imported map in module.js can't drift from node_compat.rs.
#[tokio::test]
async fn builtin_modules_matches_registry() {
    ensure_v8();
    let engine = create_test_engine();

    let expected: Vec<&str> = server::engine::node_compat::NODE_MODULES
        .iter()
        .map(|(name, _)| *name)
        .collect();
    let expected_json = serde_json::to_string(&expected).unwrap();

    let code = format!(
        r#"
        import {{ builtinModules }} from 'node:module';
        const expected = {expected_json};
        const actual = [...builtinModules].sort();
        const want = [...expected].sort();
        if (JSON.stringify(actual) !== JSON.stringify(want)) {{
            throw new Error('builtinModules drift: ' + JSON.stringify(actual) +
                ' vs registry ' + JSON.stringify(want));
        }}
        // Every listed builtin must actually import.
        for (const name of builtinModules) {{
            await import('node:' + name);
        }}
        "#
    );

    if let Err(error) = run_js(&engine, &code).await {
        panic!("execution failed: {error}");
    }
}

#[tokio::test]
async fn create_require_serves_builtins() {
    expect_ok(
        r#"
        import module, { createRequire, isBuiltin, builtinModules } from 'node:module';
        import path from 'node:path';

        const require = createRequire(import.meta.url);
        if (require('path') !== path) throw new Error('require(path) identity');
        if (require('node:path') !== path) throw new Error('node: prefix');
        if (require('crypto').createHash('sha256').update('abc').digest('hex') !==
            'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad') {
            throw new Error('require(crypto) not functional');
        }
        if (require('module') !== module) throw new Error('require(module) identity');
        if (require.resolve('url') !== 'node:url') throw new Error('require.resolve');

        try { require('./local.js'); throw new Error('file require should throw'); }
        catch (e) { if (e.code !== 'MODULE_NOT_FOUND') throw e; }
        try { require('leftpad'); throw new Error('package require should throw'); }
        catch (e) { if (e.code !== 'MODULE_NOT_FOUND') throw e; }

        if (!isBuiltin('fs') || !isBuiltin('node:fs/promises') || isBuiltin('leftpad')) {
            throw new Error('isBuiltin');
        }
        if (!builtinModules.includes('timers/promises')) throw new Error('subpath builtins listed');
        "#,
    )
    .await;
}

#[tokio::test]
async fn timers_module_and_promises() {
    expect_ok(
        r#"
        import timers from 'node:timers';
        import timersPromises, { setTimeout as sleep, setInterval as every, scheduler }
            from 'node:timers/promises';

        if (timers.promises !== timersPromises) throw new Error('timers.promises identity');

        // Module timers return Node-style handles wired to the shared registry.
        const handle = timers.setTimeout(() => { throw new Error('cleared timer fired'); }, 5);
        if (typeof handle.unref !== 'function' || typeof handle.ref !== 'function') {
            throw new Error('Timeout handle shape');
        }
        timers.clearTimeout(handle);

        const v = await sleep(5, 'value');
        if (v !== 'value') throw new Error('promisified setTimeout value: ' + v);

        // Abort pre- and post-schedule.
        const pre = new AbortController();
        pre.abort();
        await sleep(5, 'x', { signal: pre.signal }).then(
            () => { throw new Error('pre-aborted should reject'); },
            (e) => { if (e.name !== 'AbortError') throw e; });
        const post = new AbortController();
        const pending = sleep(5000, 'x', { signal: post.signal });
        post.abort();
        await pending.then(
            () => { throw new Error('post-aborted should reject'); },
            (e) => { if (e.name !== 'AbortError') throw e; });

        const im = await timersPromises.setImmediate('imm');
        if (im !== 'imm') throw new Error('promisified setImmediate value');

        let ticks = 0;
        for await (const tick of every(1, 'tick')) {
            if (tick !== 'tick') throw new Error('interval value');
            if (++ticks >= 3) break;
        }

        await scheduler.wait(1);
        await scheduler.yield();
        "#,
    )
    .await;
}

#[tokio::test]
async fn https_stub_shape() {
    expect_ok(
        r#"
        import https, { Agent, globalAgent, request } from 'node:https';
        import http from 'node:http';

        if (!(globalAgent instanceof Agent)) throw new Error('globalAgent');
        if (!(globalAgent instanceof http.Agent)) throw new Error('Agent extends http.Agent');
        try { request('https://example.com'); throw new Error('request should throw'); }
        catch (e) {
            if (!String(e.message).includes('fetch()')) {
                throw new Error('error should point at fetch(): ' + e.message);
            }
        }
        try { https.createServer(); throw new Error('createServer should throw'); }
        catch (e) { if (!/not supported/.test(e.message)) throw e; }
        "#,
    )
    .await;
}

#[tokio::test]
async fn fs_promises_stub_shape() {
    expect_ok(
        r#"
        import fsPromises, { readFile, constants } from 'node:fs/promises';
        import fs from 'node:fs';

        if (fsPromises !== fs.promises) throw new Error('fs.promises identity');
        if (constants.F_OK !== 0 || constants.R_OK !== 4) throw new Error('constants');

        // Rejecting stubs carry the Node-style code and point at the
        // policy-gated capability.
        await readFile('/etc/passwd').then(
            () => { throw new Error('readFile should reject'); },
            (e) => {
                if (e.code !== 'ENOSYS') throw new Error('code: ' + e.code);
                if (!e.message.includes('globalThis.fs')) throw new Error(e.message);
            });
        for (const name of ['writeFile', 'stat', 'mkdir', 'readdir', 'unlink', 'access']) {
            if (typeof fsPromises[name] !== 'function') throw new Error('missing ' + name);
        }
        "#,
    )
    .await;
}

#[tokio::test]
async fn stream_web_reexports_globals() {
    expect_ok(
        r#"
        import streamWeb, { ReadableStream, TransformStream, TextEncoderStream }
            from 'node:stream/web';

        if (ReadableStream !== globalThis.ReadableStream) throw new Error('ReadableStream identity');
        if (streamWeb.WritableStream !== globalThis.WritableStream) throw new Error('WritableStream identity');
        if (typeof TextEncoderStream !== 'function') throw new Error('TextEncoderStream');

        // The re-exported classes are the live ones: run a roundtrip.
        const upper = new TransformStream({
            transform(chunk, controller) { controller.enqueue(chunk.toUpperCase()); },
        });
        const reader = upper.readable.getReader();
        const writer = upper.writable.getWriter();
        writer.write('abc');
        writer.close();
        const { value } = await reader.read();
        if (value !== 'ABC') throw new Error('TransformStream roundtrip: ' + value);
        "#,
    )
    .await;
}

#[tokio::test]
async fn console_module_is_global_console() {
    expect_ok(
        r#"
        import consoleModule, { Console } from 'node:console';

        if (consoleModule !== globalThis.console) throw new Error('default identity');
        if (typeof Console !== 'function') throw new Error('Console class');
        if (!(globalThis.console instanceof Console)) throw new Error('global instanceof Console');

        const lines = [];
        const c = new Console({ write: (s) => lines.push(s) });
        c.log('n=%d', 7);
        if (lines[0] !== 'n=7\n') throw new Error('instance formatting: ' + JSON.stringify(lines));
        "#,
    )
    .await;
}
