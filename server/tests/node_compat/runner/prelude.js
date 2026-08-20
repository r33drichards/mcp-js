// Prelude for running Node.js core tests inside the mcp-v8 engine.
//
// Runs as ESM before the test body, which the Rust harness invokes through
// a CommonJS function wrapper. Provides require() over the
// node: compat registry, module/exports, __filename/__dirname, the process
// and Buffer globals, and the `../common` module with mustCall tracking.
// The harness prints a JSON result under a sentinel once timers drain.

import assert, { strict as assertStrict } from 'node:assert';
import buffer, { Buffer } from 'node:buffer';
import childProcess from 'node:child_process';
import consoleModule from 'node:console';
import crypto from 'node:crypto';
import dns from 'node:dns';
import dnsPromises from 'node:dns/promises';
import events from 'node:events';
import fs from 'node:fs';
import fsPromises from 'node:fs/promises';
import http from 'node:http';
import http2 from 'node:http2';
import https from 'node:https';
import internalEsmResolve from 'node:internal/modules/esm/resolve';
import moduleModule from 'node:module';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import perfHooks from 'node:perf_hooks';
import process from 'node:process';
import querystring from 'node:querystring';
import stream from 'node:stream';
import streamWeb from 'node:stream/web';
import test from 'node:test';
import timers from 'node:timers';
import timersPromises from 'node:timers/promises';
import tls from 'node:tls';
import url from 'node:url';
import util from 'node:util';
import utilTypes from 'node:util/types';
import zlib from 'node:zlib';

globalThis.process = process;
globalThis.Buffer = Buffer;

const failures = [];
const mustCalls = [];

const common = {
    isWindows: false,
    isLinux: true,
    isMacOS: false,
    isAIX: false,
    isIBMi: false,
    isFreeBSD: false,
    isOpenBSD: false,
    isSunOS: false,
    isDumbTerminal: false,
    isMainThread: true,
    hasCrypto: true,
    hasInspector: false,
    hasIntl: true,
    hasIPv6: false,
    enoughTestMem: true,
    buildType: 'Release',

    platformTimeout(ms) { return ms; },

    printSkipMessage(msg) { console.log('1..0 # Skipped: ' + msg); },
    skip(msg) {
        globalThis.__NODE_TEST_SKIPPED__ = String(msg || 'skipped');
        // Throwing aborts the rest of the test body; the harness treats a
        // skip marker as success.
        const err = new Error('__NODE_TEST_SKIP__');
        err.__nodeTestSkip = true;
        throw err;
    },
    skipIfInspectorDisabled() {
        if (!common.hasInspector) common.skip('V8 inspector is disabled');
    },

    mustCall(fn, exact) {
        return common._mustCallInner(fn, exact === undefined ? 1 : exact, 'exact');
    },
    mustCallAtLeast(fn, minimum) {
        return common._mustCallInner(fn, minimum === undefined ? 1 : minimum, 'minimum');
    },
    mustSucceed(fn) {
        return common.mustCall(function (err, ...args) {
            assert.ifError(err);
            if (typeof fn === 'function') return fn.call(this, ...args);
        });
    },
    _mustCallInner(fn, criteria, field) {
        if (typeof fn === 'number') { criteria = fn; fn = () => {}; }
        else if (fn === undefined) { fn = () => {}; }
        if (typeof criteria !== 'number') throw new TypeError('Invalid mustCall criteria');
        const context = {
            expected: criteria, actual: 0, kind: field,
            name: fn.name || '<anonymous>',
            stack: new Error().stack,
        };
        mustCalls.push(context);
        function wrapped(...args) {
            context.actual++;
            return fn.call(this, ...args);
        }
        wrapped.__proto__ = fn.__proto__; // eslint-disable-line no-proto
        return wrapped;
    },
    mustNotCall(msg) {
        return function mustNotCall(...args) {
            failures.push('mustNotCall() was called' + (msg ? ': ' + msg : '') +
                (args.length ? ' with args: ' + args.map((a) => String(a)).join(', ') : ''));
        };
    },

    async spawnPromisified(command, args = [], options = {}) {
        const hostCorpus = globalThis.__NODE_TEST_CORPUS_HOST__;
        const hostExecPath = globalThis.__NODE_TEST_EXEC_PATH__;
        function translate(value) {
            const text = String(value);
            if (hostCorpus && text.startsWith('/test/')) return hostCorpus + text;
            if (hostCorpus && text.startsWith('file:///test/')) {
                return 'file://' + hostCorpus + text.slice('file://'.length);
            }
            return text;
        }
        const selfHosted = command === process.execPath && hostExecPath;
        const executable = selfHosted ? hostExecPath : translate(command);
        const childArgs = selfHosted
            ? ['--node-compat-cli', ...args]
            : Array.from(args, translate);
        const output = await new Deno.Command(executable, {
            args: childArgs,
            cwd: options.cwd
                ? (selfHosted ? options.cwd : translate(options.cwd))
                : undefined,
            env: options.env,
        }).output();
        const decoder = new TextDecoder();
        function normalizeOutput(bytes) {
            let text = decoder.decode(bytes);
            if (hostCorpus) {
                const hostTestRoot = hostCorpus.replace(/\/+$/, '') + '/test';
                text = text.split(hostTestRoot).join('/test');
            }
            return text;
        }
        return {
            code: output.code,
            signal: output.signal,
            stdout: normalizeOutput(output.stdout),
            stderr: normalizeOutput(output.stderr),
        };
    },
    mustNotMutateObjectDeep(obj) { return obj; },

    invalidArgTypeHelper(input) {
        if (input == null) return ` Received ${input}`;
        if (typeof input === 'function') {
            return ` Received function ${input.name || '(anonymous)'}`;
        }
        if (typeof input === 'object') {
            if (input.constructor && input.constructor.name) {
                return ` Received an instance of ${input.constructor.name}`;
            }
            return ` Received ${util.inspect(input, { depth: -1 })}`;
        }
        let inspected = util.inspect(input, { colors: false });
        if (inspected.length > 28) inspected = inspected.slice(0, 25) + '...';
        return ` Received type ${typeof input} (${inspected})`;
    },

    expectsError(validator, exact) {
        return common.mustCall((...args) => {
            if (args.length !== 1) {
                failures.push(`expectsError: expected 1 argument, got ${args.length}`);
                return;
            }
            try {
                assert.throws(() => { throw args[0]; }, validator);
            } catch (e) {
                failures.push('expectsError: ' + e.message);
            }
            return true;
        }, exact);
    },

    expectWarning() {},
    allowGlobals() {},
    getArrayBufferViews(buf) {
        const { buffer: b, byteOffset, byteLength } = buf;
        const out = [];
        for (const T of [Int8Array, Uint8Array, Uint8ClampedArray, Int16Array,
            Uint16Array, Int32Array, Uint32Array, Float32Array, Float64Array, DataView]) {
            const bpe = T === DataView ? 1 : T.BYTES_PER_ELEMENT;
            if (byteLength % bpe === 0) {
                out.push(new T(b, byteOffset, byteLength / bpe));
            }
        }
        return out;
    },
    getBufferSources(buf) {
        return [...common.getArrayBufferViews(buf), new Uint8Array(buf).buffer];
    },
};

const modules = {
    assert: assert,
    vm: {
        // Same-realm approximation: enough for tests that only need an
        // object created "elsewhere".
        runInNewContext(code) { return (0, eval)(code); },
        runInThisContext(code) { return (0, eval)(code); },
    },
    'assert/strict': assertStrict,
    buffer: buffer,
    child_process: childProcess,
    console: consoleModule,
    crypto: crypto,
    dns: dns,
    'dns/promises': dnsPromises,
    events: events,
    fs: fs,
    'fs/promises': fsPromises,
    http: http,
    http2: http2,
    https: https,
    'internal/modules/esm/resolve': internalEsmResolve,
    module: moduleModule,
    net: net,
    os: os,
    path: path,
    perf_hooks: perfHooks,
    process: process,
    querystring: querystring,
    stream: stream,
    'stream/web': streamWeb,
    test: test,
    timers: timers,
    'timers/promises': timersPromises,
    tls: tls,
    url: url,
    util: util,
    'util/types': utilTypes,
    zlib: zlib,
};

const fixtures = {
    fixturesDir: '/test/fixtures',
    path: (...args) => path.join('/test/fixtures', ...args),
    fileURL: (...args) => url.pathToFileURL(path.join('/test/fixtures', ...args)),
};

globalThis.__NODE_TEST_COMMON__ = common;
globalThis.__NODE_TEST_PENDING__ = 0;
globalThis.__NODE_TEST_RECORD_FAILURE__ = function recordFailure(error) {
    failures.push(error && error.stack ? error.stack : String(error));
};
globalThis.__NODE_TEST_FIXTURES__ = fixtures;

const testPath = '/' + (globalThis.__NODE_TEST_PATH__ || ('test/parallel/' + (globalThis.__NODE_TEST_NAME__ || 'test.js')));
const testDir = testPath.slice(0, testPath.lastIndexOf('/')) || '/';
const virtualRequire = moduleModule.createRequire(testPath);

function nodeRequire(id) {
    let name = String(id);
    if (name.startsWith('node:')) name = name.slice(5);
    if (name === '../common' || name === '../common/index.js') return common;
    if (name === '../common/fixtures' || name === '../common/fixtures.js') {
        return fixtures;
    }
    if (name.startsWith('../common/')) {
        throw new Error('Unsupported common submodule: ' + name);
    }
    if (Object.prototype.hasOwnProperty.call(modules, name)) return modules[name];
    return virtualRequire(id);
}
nodeRequire.resolve = virtualRequire.resolve;
nodeRequire.cache = virtualRequire.cache;
nodeRequire.main = virtualRequire.main;

// The harness schedules its drain-time report through this stash so tests
// that delete the timer globals (test-timers-api-refs) can still report.
globalThis.__NODE_TEST_SETTIMEOUT__ = globalThis.setTimeout;

globalThis.__NODE_TEST_RUN_CJS__ = function runCommonJS(source) {
    const testModule = { exports: {} };
    const compiled = Function(
        'exports', 'require', 'module', '__filename', '__dirname', String(source),
    );
    return compiled.call(
        testModule.exports, testModule.exports, nodeRequire, testModule, testPath, testDir,
    );
};

globalThis.__NODE_TEST_SCHEDULE_REPORT__ = function scheduleReport(sentinel) {
    function scheduleCheck(delay) {
        const timer = globalThis.__NODE_TEST_SETTIMEOUT__(check, delay);
        globalThis.__mcpV8SetTimerResourceTracked(timer, false);
    }
    function check() {
        const active = globalThis.__mcpV8GetActiveResourcesInfo();
        if (active.length > 0 || globalThis.__NODE_TEST_PENDING__ > 0) {
            scheduleCheck(25);
            return;
        }
        console.log(String(sentinel) + globalThis.__NODE_TEST_REPORT__());
    }
    // Preserve the original settle window for promise-only work, then wait
    // until all referenced resources supported by the runtime have drained.
    scheduleCheck(300);
};

globalThis.__NODE_TEST_REPORT__ = function () {
    for (const c of mustCalls) {
        const ok = c.kind === 'exact' ? c.actual === c.expected : c.actual >= c.expected;
        if (!ok) {
            const callsite = String(c.stack || '')
                .split('\n')
                .find((line) => line.includes('file:///test/'));
            failures.push(
                `mustCall(${c.name}${callsite ? ` ${callsite.trim()}` : ''}): expected ` +
                `${c.kind === 'exact' ? '' : '>= '}${c.expected} calls, got ${c.actual}`);
        }
    }
    return JSON.stringify({
        skipped: globalThis.__NODE_TEST_SKIPPED__ || null,
        failures: failures,
    });
};
