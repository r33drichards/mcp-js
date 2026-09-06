// Prelude for running Node.js core tests inside the mcp-v8 engine.
//
// Runs as ESM before the (CJS) test body, which is evaluated via indirect
// eval by the Rust harness. Provides the CJS shell: require() over the
// node: compat registry, module/exports, __filename/__dirname, the process
// and Buffer globals, and the `../common` module with mustCall tracking.
// The harness prints a JSON result under a sentinel once timers drain.

import assert, { strict as assertStrict } from 'node:assert';
import buffer, { Buffer } from 'node:buffer';
import consoleModule from 'node:console';
import crypto from 'node:crypto';
import events from 'node:events';
import moduleModule from 'node:module';
import os from 'node:os';
import path from 'node:path';
import process from 'node:process';
import querystring from 'node:querystring';
import stream from 'node:stream';
import timers from 'node:timers';
import timersPromises from 'node:timers/promises';
import url from 'node:url';
import util from 'node:util';

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
    console: consoleModule,
    crypto: crypto,
    events: events,
    module: moduleModule,
    os: os,
    path: path,
    process: process,
    querystring: querystring,
    stream: stream,
    timers: timers,
    'timers/promises': timersPromises,
    url: url,
    util: util,
};

globalThis.require = function require(id) {
    let name = String(id);
    if (name.startsWith('node:')) name = name.slice(5);
    if (name === '../common' || name === '../common/index.js') return common;
    if (name === '../common/fixtures') {
        return {
            fixturesDir: '/test/fixtures',
            path: (...args) => ['/test/fixtures', ...args].join('/'),
        };
    }
    if (name.startsWith('../common/')) {
        throw new Error('Unsupported common submodule: ' + name);
    }
    if (Object.prototype.hasOwnProperty.call(modules, name)) return modules[name];
    const err = new Error("Cannot find module '" + id + "'");
    err.code = 'MODULE_NOT_FOUND';
    throw err;
};

// The harness schedules its drain-time report through this stash so tests
// that delete the timer globals (test-timers-api-refs) can still report.
globalThis.__NODE_TEST_SETTIMEOUT__ = globalThis.setTimeout;

globalThis.module = { exports: {} };
globalThis.exports = globalThis.module.exports;
globalThis.__filename = '/test/parallel/' + (globalThis.__NODE_TEST_NAME__ || 'test.js');
globalThis.__dirname = '/test/parallel';

globalThis.__NODE_TEST_REPORT__ = function () {
    for (const c of mustCalls) {
        const ok = c.kind === 'exact' ? c.actual === c.expected : c.actual >= c.expected;
        if (!ok) {
            failures.push(
                `mustCall(${c.name}): expected ${c.kind === 'exact' ? '' : '>= '}` +
                `${c.expected} calls, got ${c.actual}`);
        }
    }
    return JSON.stringify({
        skipped: globalThis.__NODE_TEST_SKIPPED__ || null,
        failures: failures,
    });
};
