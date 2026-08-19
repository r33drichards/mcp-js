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
        ("node:perf_hooks", "perf_hooks: perfHooks"),
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
        globalThis.__NODE_TEST_RUN_CJS__(`
            const commonHarness = require('../common');
            if (commonHarness.hasInspector !== false) throw new Error('inspector must be disabled');
            try {{
                commonHarness.skipIfInspectorDisabled();
                throw new Error('expected inspector skip');
            }} catch (error) {{
                if (!error.__nodeTestSkip) throw error;
            }}
        `);
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
async fn process_active_resources() {
    expect_ok(
        r#"
        import process, { getActiveResourcesInfo } from 'node:process';

        if (process.getActiveResourcesInfo !== getActiveResourcesInfo) {
            throw new Error('named export identity');
        }
        const initial = getActiveResourcesInfo();
        if (!Array.isArray(initial) || initial.length !== 0) {
            throw new Error('initial resources: ' + JSON.stringify(initial));
        }
        initial.push('fake');
        if (getActiveResourcesInfo().includes('fake')) {
            throw new Error('resource snapshots must be fresh');
        }

        const timeout = setTimeout(() => {}, 5000);
        if (!getActiveResourcesInfo().includes('Timeout')) {
            throw new Error('timeout resource missing');
        }
        timeout.unref();
        if (getActiveResourcesInfo().includes('Timeout')) {
            throw new Error('unref timeout must be hidden');
        }
        timeout.ref();
        if (!getActiveResourcesInfo().includes('Timeout')) {
            throw new Error('ref timeout must be restored');
        }
        clearTimeout(timeout);
        if (getActiveResourcesInfo().includes('Timeout')) {
            throw new Error('cleared timeout must be removed');
        }

        const interval = setInterval(() => {}, 5000);
        if (!getActiveResourcesInfo().includes('Timeout')) {
            throw new Error('interval resource missing');
        }
        clearInterval(interval);

        const immediate = setImmediate(() => {});
        if (!getActiveResourcesInfo().includes('Immediate')) {
            throw new Error('immediate resource missing');
        }
        clearImmediate(immediate);
        if (getActiveResourcesInfo().includes('Immediate')) {
            throw new Error('cleared immediate must be removed');
        }
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

#[tokio::test]
async fn crypto_random_size_contracts() {
    expect_ok(
        r#"
        import crypto, { pseudoRandomBytes, randomBytes, randomFillSync } from 'node:crypto';

        function expectError(value, Type, code, message) {
            try {
                randomBytes(value);
                throw new Error('expected ' + code);
            } catch (error) {
                if (!(error instanceof Type)) throw error;
                if (error.code !== code) throw new Error('wrong code: ' + error.code);
                if (error.message !== message) throw new Error('wrong message: ' + error.message);
            }
        }

        for (const [value, received] of [
            [undefined, 'undefined'], [null, 'null'], [false, 'type boolean (false)'],
            [true, 'type boolean (true)'], [{}, 'an instance of Object'],
            [[], 'an instance of Array'],
        ]) {
            expectError(
                value, TypeError, 'ERR_INVALID_ARG_TYPE',
                'The "size" argument must be of type number. Received ' + received,
            );
        }
        for (const value of [-1, NaN, 2 ** 31, 2 ** 32]) {
            expectError(
                value, RangeError, 'ERR_OUT_OF_RANGE',
                'The value of "size" is out of range. It must be >= 0 && <= 2147483647. Received ' + value,
            );
        }
        if (randomBytes(101.2).length !== 101) throw new Error('fractional size');
        if (typeof pseudoRandomBytes !== 'function' || pseudoRandomBytes(2).length !== 2) {
            throw new Error('pseudoRandomBytes alias');
        }
        for (const name of ['pseudoRandomBytes', 'prng', 'rng']) {
            const descriptor = Object.getOwnPropertyDescriptor(crypto, name);
            if (!descriptor || descriptor.value !== randomBytes ||
                descriptor.configurable !== true || descriptor.enumerable !== false) {
                throw new Error('legacy alias descriptor: ' + name);
            }
        }
        for (const callback of [1, true, NaN, null, {}, []]) {
            try { randomBytes(1, callback); throw new Error('expected callback type'); }
            catch (error) { if (error.code !== 'ERR_INVALID_ARG_TYPE') throw error; }
        }
        const raw = new ArrayBuffer(8);
        const returned = randomFillSync(raw);
        if (returned !== raw || new Uint8Array(raw).every((value) => value === 0)) {
            throw new Error('ArrayBuffer randomFillSync');
        }
        expectError(
            'test', TypeError, 'ERR_INVALID_ARG_TYPE',
            'The "size" argument must be of type number. Received type string (\'test\')',
        );
        try { randomFillSync(new Uint8Array(10), 'test'); throw new Error('expected offset type'); }
        catch (error) {
            if (error.code !== 'ERR_INVALID_ARG_TYPE' ||
                error.message !== 'The "offset" argument must be of type number. Received type string (\'test\')') {
                throw error;
            }
        }
        try { randomFillSync(new Uint8Array(10), 1, 10); throw new Error('expected size + offset'); }
        catch (error) {
            if (error.code !== 'ERR_OUT_OF_RANGE' ||
                error.message !== 'The value of "size + offset" is out of range. It must be <= 10. Received 11') {
                throw error;
            }
        }
        "#,
    )
    .await;
}

#[tokio::test]
async fn crypto_random_int_contracts() {
    expect_ok(
        r#"
        import { randomInt } from 'node:crypto';

        function expectError(run, Type, code, message) {
            try {
                run();
                throw new Error('expected ' + code);
            } catch (error) {
                if (!(error instanceof Type)) throw error;
                if (error.code !== code) throw new Error('wrong code: ' + error.code);
                if (error.message !== message) throw new Error('wrong message: ' + error.message);
            }
        }

        expectError(
            () => randomInt('10', 100), TypeError, 'ERR_INVALID_ARG_TYPE',
            'The "min" argument must be a safe integer. Received type string (\'10\')',
        );
        expectError(
            () => randomInt('10'), TypeError, 'ERR_INVALID_ARG_TYPE',
            'The "max" argument must be a safe integer. Received type string (\'10\')',
        );
        expectError(
            () => randomInt(Number.MIN_SAFE_INTEGER - 1, Number.MIN_SAFE_INTEGER + 5),
            TypeError, 'ERR_INVALID_ARG_TYPE',
            'The "min" argument must be a safe integer. Received type number (-9007199254740992)',
        );
        expectError(
            () => randomInt(3, 2), RangeError, 'ERR_OUT_OF_RANGE',
            'The value of "max" is out of range. It must be greater than the value of "min" (3). Received 2',
        );
        expectError(
            () => randomInt(1, 0xFFFF_FFFF_FFFF + 2), RangeError, 'ERR_OUT_OF_RANGE',
            'The value of "max - min" is out of range. It must be <= 281474976710655. Received 281_474_976_710_656',
        );
        expectError(
            () => randomInt(0xFFFF_FFFF_FFFF + 1), RangeError, 'ERR_OUT_OF_RANGE',
            'The value of "max" is out of range. It must be <= 281474976710655. Received 281_474_976_710_656',
        );
        expectError(
            () => randomInt(0, 1, 10), TypeError, 'ERR_INVALID_ARG_TYPE',
            'The "callback" argument must be of type function. Received type number (10)',
        );
        "#,
    )
    .await;
}

#[tokio::test]
async fn zlib_crc32_matches_node() {
    expect_ok(
        r#"
        import zlib, { crc32 } from 'node:zlib';
        import { Buffer } from 'node:buffer';

        if (zlib.crc32 !== crc32) throw new Error('crc32 export identity');
        if (crc32('') !== 0) throw new Error('empty crc32');
        if (crc32('hello') !== 0x3610a686) throw new Error('string crc32');
        if (crc32(Buffer.from('test')) !== 0xd87f7e0c) throw new Error('buffer crc32');
        if (crc32('abacus', 0x7a30360d) !== 0xf8655a84) {
            throw new Error('seeded crc32');
        }
        const view = new DataView(new Uint8Array([0x74, 0x65, 0x73, 0x74]).buffer);
        if (crc32(view) !== 0xd87f7e0c) throw new Error('DataView crc32');

        for (const invalid of [undefined, null, true, 1, () => {}, {}]) {
            try { crc32(invalid); throw new Error('expected invalid data'); }
            catch (error) { if (error.code !== 'ERR_INVALID_ARG_TYPE') throw error; }
        }
        for (const invalid of [null, true, () => {}, {}]) {
            try { crc32('test', invalid); throw new Error('expected invalid seed'); }
            catch (error) { if (error.code !== 'ERR_INVALID_ARG_TYPE') throw error; }
        }
        "#,
    )
    .await;
}

#[tokio::test]
async fn buffer_error_contracts() {
    expect_ok(
        r#"
        import { Buffer, SlowBuffer, kMaxLength } from 'node:buffer';

        function expectError(fn, Type, code, message) {
            try {
                fn();
                throw new Error('expected ' + code);
            } catch (error) {
                if (!(error instanceof Type)) throw error;
                if (error.code !== code) {
                    throw new Error('wrong code: ' + error.code + ' for ' + error.message);
                }
                if (error.message !== message && !error.message.startsWith(message)) {
                    throw new Error('wrong message: ' + error.message);
                }
            }
        }

        for (const allocate of [Buffer.allocUnsafeSlow, SlowBuffer]) {
            for (const size of [undefined, '1', {}, true]) {
                expectError(
                    () => allocate(size),
                    TypeError,
                    'ERR_INVALID_ARG_TYPE',
                    'The "size" argument must be of type number',
                );
            }
            for (const size of [NaN, Infinity, -Infinity, -1, kMaxLength + 1]) {
                expectError(
                    () => allocate(size),
                    RangeError,
                    'ERR_OUT_OF_RANGE',
                    'The value of "size" is out of range.',
                );
            }
        }
        if (Buffer.allocUnsafeSlow(2.9).length !== 2) {
            throw new Error('fractional sizes must truncate');
        }

        const base64url = Buffer.from([0xfb, 0xff]).toString('base64url');
        if (base64url !== '-_8') throw new Error('base64url encode: ' + base64url);
        if (Buffer.from(base64url, 'base64url').toString('hex') !== 'fbff') {
            throw new Error('base64url decode');
        }

        const unknownEncoding = 'Unknown encoding: nope';
        expectError(() => Buffer.from('x', 'nope'), TypeError, 'ERR_UNKNOWN_ENCODING', unknownEncoding);
        expectError(() => Buffer.from('x').toString('nope'), TypeError, 'ERR_UNKNOWN_ENCODING', unknownEncoding);
        expectError(() => Buffer.alloc(4).write('x', 'nope'), TypeError, 'ERR_UNKNOWN_ENCODING', unknownEncoding);
        expectError(() => Buffer.alloc(4).fill('x', 'nope'), TypeError, 'ERR_UNKNOWN_ENCODING', unknownEncoding);
        "#,
    )
    .await;
}

#[tokio::test]
async fn buffer_node_owned_methods() {
    expect_ok(
        r#"
        import { Buffer } from 'node:buffer';

        const methods = [
            'asciiSlice', 'base64Slice', 'base64urlSlice', 'latin1Slice',
            'hexSlice', 'ucs2Slice', 'utf8Slice', 'asciiWrite', 'base64Write',
            'base64urlWrite', 'latin1Write', 'hexWrite', 'ucs2Write',
            'utf8Write', 'subarray',
        ];
        for (const method of methods) {
            if (!Object.prototype.hasOwnProperty.call(Buffer.prototype, method) ||
                typeof Buffer.prototype[method] !== 'function') {
                throw new Error('missing Buffer.prototype.' + method);
            }
        }

        const source = Buffer.from([0x61, 0x62, 0x63]);
        if (source.asciiSlice(0, 3) !== 'abc') throw new Error('asciiSlice');
        if (source.hexSlice(0, 3) !== '616263') throw new Error('hexSlice');
        if (source.base64urlSlice(0, 3) !== 'YWJj') throw new Error('base64urlSlice');
        const view = source.subarray(1);
        if (!Buffer.isBuffer(view) || view.toString() !== 'bc') throw new Error('subarray');

        const target = Buffer.alloc(3);
        if (target.utf8Write('abc', 0, 3) !== 3 || target.toString() !== 'abc') {
            throw new Error('utf8Write');
        }

        const destination = new Uint8Array(2);
        Buffer.prototype.copy.call(source, destination, 0, 1, 3);
        if (destination[0] !== 0x62 || destination[1] !== 0x63) {
            throw new Error('generic copy');
        }
        if (!Buffer.prototype.equals.call(new Uint8Array([1, 2]), new Uint8Array([1, 2]))) {
            throw new Error('generic equals');
        }
        const utf16 = Uint8Array.of(0x9a, 0x03, 0x91, 0x03);
        if (!Buffer.prototype.includes.call(utf16, '\u039A', 0, 'utf16le')) {
            throw new Error('generic utf16 includes');
        }
        const genericSlice = Buffer.prototype.slice.call(new Uint8Array([1, 2, 3]), 1);
        if (Buffer.isBuffer(genericSlice) || genericSlice.length !== 2 || genericSlice[0] !== 2) {
            throw new Error('generic slice');
        }
        const integerTarget = new Uint8Array(1);
        Buffer.prototype.writeInt8.call(integerTarget, -123, 0);
        if (integerTarget[0] !== 133) throw new Error('generic integer write');
        const customInspect = Buffer.prototype[Symbol.for('nodejs.util.inspect.custom')];
        if (customInspect.call(new Uint8Array([1, 2])) !== '<Uint8Array 01 02>') {
            throw new Error('generic inspect');
        }
        "#,
    )
    .await;
}

#[tokio::test]
async fn perf_hooks_observer_and_timerify() {
    expect_ok(
        r#"
        import perfHooks, {
            PerformanceObserver, PerformanceObserverEntryList, performance, timerify,
        } from 'node:perf_hooks';

        if (perfHooks.performance !== globalThis.performance || performance !== globalThis.performance) {
            throw new Error('performance identity');
        }
        if (performance.timerify !== timerify) throw new Error('timerify identity');
        if (!PerformanceObserver.supportedEntryTypes.includes('measure') ||
            !PerformanceObserver.supportedEntryTypes.includes('function')) {
            throw new Error('supported entry types');
        }

        for (const value of [1, {}, [], null, undefined, Infinity]) {
            try { timerify(value); throw new Error('expected invalid fn'); }
            catch (error) { if (error.code !== 'ERR_INVALID_ARG_TYPE') throw error; }
        }
        for (const histogram of [1, '', {}, [], false]) {
            try { timerify(() => {}, { histogram }); throw new Error('expected invalid histogram'); }
            catch (error) { if (error.code !== 'ERR_INVALID_ARG_TYPE') throw error; }
        }

        const batches = [];
        const observer = new PerformanceObserver((list, self) => {
            if (!(list instanceof PerformanceObserverEntryList) || self !== observer) {
                throw new Error('observer callback shape');
            }
            batches.push(list.getEntries());
        });
        observer.observe({ entryTypes: ['measure', 'function'] });
        performance.mark('perf-start');
        performance.measure('perf-measure', 'perf-start');
        const wrapped = timerify(function add(a, b) { return a + b; });
        if (wrapped(2, 3) !== 5 || wrapped.length !== 2 || wrapped.name !== 'timerified add') {
            throw new Error('timerify wrapper semantics');
        }
        class TimedClass {}
        const WrappedClass = timerify(TimedClass);
        if (!(new WrappedClass(1, 'abc') instanceof TimedClass)) {
            throw new Error('timerify constructor semantics');
        }
        try {
            timerify(() => { throw new Error('timerify-error'); })();
            throw new Error('timerified throw must propagate');
        } catch (error) {
            if (error.message !== 'timerify-error') throw error;
        }
        await new Promise((resolve) => setImmediate(resolve));
        const entries = batches.flat();
        if (!entries.some((entry) => entry.name === 'perf-measure' && entry.entryType === 'measure')) {
            throw new Error('measure entry missing');
        }
        if (!entries.some((entry) =>
            entry.name === 'add' && entry.entryType === 'function' && entry[0] === 2 && entry[1] === 3)) {
            throw new Error('function entry missing');
        }
        if (!entries.some((entry) =>
            entry.name === 'TimedClass' && entry[0] === 1 && entry[1] === 'abc')) {
            throw new Error('constructor entry missing');
        }
        observer.disconnect();
        "#,
    )
    .await;
}
