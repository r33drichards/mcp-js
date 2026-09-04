// Prelude for running Node.js core tests inside the mcp-v8 engine.
//
// Runs as ESM before the test body, which the Rust harness invokes through
// a CommonJS function wrapper. Provides require() over the
// node: compat registry, module/exports, __filename/__dirname, the process
// and Buffer globals, and the `../common` module with mustCall tracking.
// The harness prints a JSON result under a sentinel once timers drain.

import assert, { strict as assertStrict } from 'node:assert';
import asyncHooks from 'node:async_hooks';
import buffer, { Buffer } from 'node:buffer';
import childProcess from 'node:child_process';
import consoleModule from 'node:console';
import crypto from 'node:crypto';
import diagnosticsChannel from 'node:diagnostics_channel';
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
globalThis.global = globalThis;
globalThis.__NODE_COMPAT_RESOLVE_IMPORT__ = (specifier, parentURL) =>
    globalThis.__mcpV8ResolveImportSpecifier(specifier, parentURL);
globalThis.__mcpV8ModuleHooks ??= [];
globalThis.__NODE_COMPAT_IMPORT_META_RESOLVE__ =
    globalThis.__mcpV8ImportMetaResolve;

const failures = [];
const mustCalls = [];

const common = {
    PORT: 12346,
    localhostIPv4: '127.0.0.1',
    localhostIPv6: '::1',
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
    hasQuic: false,
    hasSQLite: false,
    enoughTestMem: true,
    buildType: 'Release',
    canCreateSymLink() { return true; },

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
    mustSucceed(fn, exact) {
        return common.mustCall(function (err, ...args) {
            assert.ifError(err);
            if (typeof fn === 'function') return fn.call(this, ...args);
        }, exact);
    },
    expectsError(validator, exact) {
        return common.mustCall((...args) => {
            if (args.length !== 1) {
                assert.fail(`Expected one argument, got ${JSON.stringify(args)}`);
            }
            const error = args[0];
            assert.strictEqual(
                Object.prototype.propertyIsEnumerable.call(error, 'message'),
                false,
            );
            assert.throws(() => { throw error; }, validator);
            return true;
        }, exact);
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
        const commandEnv = selfHosted && options.cwd
            ? { ...options.env, NODE_COMPAT_FILE_ROOT: globalThis.__NODE_TEST_TMPDIR__ }
            : options.env;
        const output = await new Deno.Command(executable, {
            args: childArgs,
            cwd: options.cwd
                ? (selfHosted ? options.cwd : translate(options.cwd))
                : undefined,
            env: commandEnv,
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
            return ` Received function ${input.name}`;
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

    expectRequiredModule(mod, expectation, checkESModule = true) {
        const clone = { ...mod };
        if (Object.hasOwn(mod, 'default') && checkESModule) {
            assert.strictEqual(mod.__esModule, true);
            delete clone.__esModule;
        }
        assert(utilTypes.isModuleNamespaceObject(mod));
        assert.deepStrictEqual(clone, { ...expectation });
    },

    expectRequireStack(output, expected) {
        const lines = String(output).replace(/\r/g, '').split('\n');
        const start = lines.indexOf('Require stack:');
        if (start === -1) {
            assert.deepStrictEqual([], expected);
            return;
        }
        const stack = [];
        for (let i = start + 1; i < lines.length && lines[i].startsWith('- '); i++) {
            stack.push(lines[i].slice(2));
        }
        assert.deepStrictEqual(stack, expected);
    },

    expectRequiredTLAError(err, stack) {
        const message = /require\(\) cannot be used on an ESM graph with top-level await/;
        if (typeof err === 'string') {
            assert.match(err, /ERR_REQUIRE_ASYNC_MODULE/);
            assert.match(err, message);
            if (stack) common.expectRequireStack(err, stack);
        } else {
            assert.strictEqual(err.code, 'ERR_REQUIRE_ASYNC_MODULE');
            assert.match(err.message, message);
            if (stack) assert.deepStrictEqual(err.requireStack, stack);
        }
    },

    escapePOSIXShell(cmdParts, ...args) {
        // POSIX-only port: pass interpolated values through the environment.
        const env = { ...process.env };
        let cmd = cmdParts[0];
        for (let i = 0; i < args.length; i++) {
            const envVarName = `ESCAPED_${i}`;
            env[envVarName] = args[i];
            cmd += '${' + envVarName + '}' + cmdParts[i + 1];
        }
        return [cmd, { env }];
    },

    skipIfEslintMissing() {
        common.skip('missing ESLint');
    },
    skipIfSQLiteMissing() {
        common.skip('missing SQLite');
    },
    skipIf32Bits() {},

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
    async_hooks: asyncHooks,
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
    diagnostics_channel: diagnosticsChannel,
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

// Port of test/common/child_process.js over the node:child_process compat
// module. spawnSync self-hosts process.execPath children and translates
// corpus paths, so the upstream logic carries over unchanged.
const commonChildProcess = (() => {
    const { spawnSync } = childProcess;

    function cleanupStaleProcess() {}

    const kExpiringChildRunTime = common.platformTimeout(20 * 1000);
    const kExpiringParentTimer = 1;

    function logAfterTime(time) {
        setTimeout(() => {
            // The following console statements are part of the test.
            console.log('child stdout');
            console.error('child stderr');
        }, time);
    }

    function checkOutput(str, check) {
        if ((check instanceof RegExp && !check.test(str)) ||
            (typeof check === 'string' && check !== str)) {
            return { passed: false, reason: `did not match ${util.inspect(check)}` };
        }
        if (typeof check === 'function') {
            try {
                check(str);
            } catch (error) {
                return {
                    passed: false,
                    reason: `did not match expectation, checker throws:\n${util.inspect(error)}`,
                };
            }
        }
        return { passed: true };
    }

    function expectSyncExit(caller, spawnArgs, {
        status,
        signal,
        stderr: stderrCheck,
        stdout: stdoutCheck,
        trim = false,
    }) {
        const child = spawnSync(...spawnArgs);
        const checkFailures = [];
        let stderrStr, stdoutStr;
        if (status !== undefined && child.status !== status) {
            checkFailures.push(`- process terminated with status ${child.status}, expected ${status}`);
        }
        if (signal !== undefined && child.signal !== signal) {
            checkFailures.push(`- process terminated with signal ${child.signal}, expected ${signal}`);
        }

        function logAndThrow() {
            const tag = `[process ${child.pid}]:`;
            console.error(`${tag} --- stderr ---`);
            console.error(stderrStr === undefined ? child.stderr.toString() : stderrStr);
            console.error(`${tag} --- stdout ---`);
            console.error(stdoutStr === undefined ? child.stdout.toString() : stdoutStr);
            console.error(`${tag} status = ${child.status}, signal = ${child.signal}`);

            const error = new Error(`${checkFailures.join('\n')}`);
            if (typeof spawnArgs[2] === 'object' && spawnArgs[2] !== null) {
                const envInOptions = spawnArgs[2].env;
                if (typeof envInOptions === 'object' && envInOptions !== null &&
                    envInOptions !== process.env) {
                    error.options = { ...spawnArgs[2], env: {} };
                    for (const key of Object.keys(envInOptions)) {
                        if (envInOptions[key] !== process.env[key]) {
                            error.options.env[key] = spawnArgs[2].env[key];
                        }
                    }
                } else {
                    error.options = spawnArgs[2];
                }
            }
            let command = spawnArgs[0];
            if (Array.isArray(spawnArgs[1])) {
                command += ' ' + spawnArgs[1].join(' ');
            }
            error.command = command;
            Error.captureStackTrace(error, caller);
            throw error;
        }

        if (checkFailures.length !== 0) logAndThrow();

        if (stderrCheck !== undefined) {
            stderrStr = child.stderr.toString();
            const { passed, reason } = checkOutput(trim ? stderrStr.trim() : stderrStr, stderrCheck);
            if (!passed) checkFailures.push(`- stderr ${reason}`);
        }
        if (stdoutCheck !== undefined) {
            stdoutStr = child.stdout.toString();
            const { passed, reason } = checkOutput(trim ? stdoutStr.trim() : stdoutStr, stdoutCheck);
            if (!passed) checkFailures.push(`- stdout ${reason}`);
        }
        if (checkFailures.length !== 0) logAndThrow();
        return { child, stderr: stderrStr, stdout: stdoutStr };
    }

    function spawnSyncAndExit(...args) {
        const spawnArgs = args.slice(0, args.length - 1);
        const expectations = args[args.length - 1];
        return expectSyncExit(spawnSyncAndExit, spawnArgs, expectations);
    }

    function spawnSyncAndExitWithoutError(...args) {
        return expectSyncExit(spawnSyncAndExitWithoutError, [...args], {
            status: 0,
            signal: null,
        });
    }

    function spawnSyncAndAssert(...args) {
        const expectations = args.pop();
        return expectSyncExit(spawnSyncAndAssert, [...args], {
            status: 0,
            signal: null,
            ...expectations,
        });
    }

    return {
        cleanupStaleProcess,
        logAfterTime,
        kExpiringChildRunTime,
        kExpiringParentTimer,
        spawnSyncAndAssert,
        spawnSyncAndExit,
        spawnSyncAndExitWithoutError,
    };
})();

// Port of test/common/countdown.js.
const kCountdownLimit = Symbol('limit');
const kCountdownCallback = Symbol('callback');
class Countdown {
    constructor(limit, cb) {
        assert.strictEqual(typeof limit, 'number');
        assert.strictEqual(typeof cb, 'function');
        this[kCountdownLimit] = limit;
        this[kCountdownCallback] = common.mustCall(cb);
    }

    dec() {
        assert(this[kCountdownLimit] > 0, 'Countdown expired');
        if (--this[kCountdownLimit] === 0) this[kCountdownCallback]();
        return this[kCountdownLimit];
    }

    get remaining() {
        return this[kCountdownLimit];
    }
}

// Port of test/common/crypto.js over the node:crypto compat module. The
// OpenSSL version probe tolerates a runtime without process.versions.openssl.
const commonCrypto = (() => {
    const { createSign, createVerify, publicEncrypt, privateDecrypt, sign, verify } = crypto;

    const modp2buf = Buffer.from([
        0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xc9, 0x0f,
        0xda, 0xa2, 0x21, 0x68, 0xc2, 0x34, 0xc4, 0xc6, 0x62, 0x8b,
        0x80, 0xdc, 0x1c, 0xd1, 0x29, 0x02, 0x4e, 0x08, 0x8a, 0x67,
        0xcc, 0x74, 0x02, 0x0b, 0xbe, 0xa6, 0x3b, 0x13, 0x9b, 0x22,
        0x51, 0x4a, 0x08, 0x79, 0x8e, 0x34, 0x04, 0xdd, 0xef, 0x95,
        0x19, 0xb3, 0xcd, 0x3a, 0x43, 0x1b, 0x30, 0x2b, 0x0a, 0x6d,
        0xf2, 0x5f, 0x14, 0x37, 0x4f, 0xe1, 0x35, 0x6d, 0x6d, 0x51,
        0xc2, 0x45, 0xe4, 0x85, 0xb5, 0x76, 0x62, 0x5e, 0x7e, 0xc6,
        0xf4, 0x4c, 0x42, 0xe9, 0xa6, 0x37, 0xed, 0x6b, 0x0b, 0xff,
        0x5c, 0xb6, 0xf4, 0x06, 0xb7, 0xed, 0xee, 0x38, 0x6b, 0xfb,
        0x5a, 0x89, 0x9f, 0xa5, 0xae, 0x9f, 0x24, 0x11, 0x7c, 0x4b,
        0x1f, 0xe6, 0x49, 0x28, 0x66, 0x51, 0xec, 0xe6, 0x53, 0x81,
        0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    ]);

    function assertApproximateSize(key, expectedSize) {
        const u = typeof key === 'string' ? 'chars' : 'bytes';
        const min = Math.floor(0.9 * expectedSize);
        const max = Math.ceil(1.1 * expectedSize);
        assert(key.length >= min,
               `Key (${key.length} ${u}) is shorter than expected (${min} ${u})`);
        assert(key.length <= max,
               `Key (${key.length} ${u}) is longer than expected (${max} ${u})`);
    }

    function testEncryptDecrypt(publicKey, privateKey) {
        const message = 'Hello Node.js world!';
        const plaintext = Buffer.from(message, 'utf8');
        for (const key of [publicKey, privateKey]) {
            const ciphertext = publicEncrypt(key, plaintext);
            const received = privateDecrypt(privateKey, ciphertext);
            assert.strictEqual(received.toString('utf8'), message);
        }
    }

    function testSignVerify(publicKey, privateKey) {
        const message = Buffer.from('Hello Node.js world!');

        function oldSign(algo, data, key) {
            return createSign(algo).update(data).sign(key);
        }

        function oldVerify(algo, data, key, signature) {
            return createVerify(algo).update(data).verify(key, signature);
        }

        for (const signFn of [sign, oldSign]) {
            const signature = signFn('SHA256', message, privateKey);
            for (const verifyFn of [verify, oldVerify]) {
                for (const key of [publicKey, privateKey]) {
                    const okay = verifyFn('SHA256', message, key, signature);
                    assert(okay);
                }
            }
        }
    }

    function getRegExpForPEM(label, cipher) {
        const head = `\\-\\-\\-\\-\\-BEGIN ${label}\\-\\-\\-\\-\\-`;
        const rfc1421Header = cipher == null ? '' :
            `\nProc-Type: 4,ENCRYPTED\nDEK-Info: ${cipher},[^\n]+\n`;
        const body = '([a-zA-Z0-9\\+/=]{64}\n)*[a-zA-Z0-9\\+/=]{1,64}';
        const end = `\\-\\-\\-\\-\\-END ${label}\\-\\-\\-\\-\\-`;
        return new RegExp(`^${head}${rfc1421Header}\n${body}\n${end}\n$`);
    }

    const opensslVersionNumber = (major = 0, minor = 0, patch = 0) => {
        assert(major >= 0 && major <= 0xf);
        assert(minor >= 0 && minor <= 0xff);
        assert(patch >= 0 && patch <= 0xff);
        return (major << 28) | (minor << 20) | (patch << 4);
    };

    let OPENSSL_VERSION_NUMBER;
    const hasOpenSSL = (major = 0, minor = 0, patch = 0) => {
        if (!common.hasCrypto) return false;
        if (OPENSSL_VERSION_NUMBER === undefined) {
            const version = process.versions.openssl;
            const groups = typeof version === 'string'
                ? version.match(/(?<m>\d+)\.(?<n>\d+)\.(?<p>\d+)/)?.groups
                : undefined;
            if (!groups) return false;
            OPENSSL_VERSION_NUMBER =
                opensslVersionNumber(groups.m, groups.n, groups.p);
        }
        return OPENSSL_VERSION_NUMBER >= opensslVersionNumber(major, minor, patch);
    };

    return {
        modp2buf,
        assertApproximateSize,
        testEncryptDecrypt,
        testSignVerify,
        pkcs1PubExp: getRegExpForPEM('RSA PUBLIC KEY'),
        pkcs1PrivExp: getRegExpForPEM('RSA PRIVATE KEY'),
        pkcs1EncExp: (cipher) => getRegExpForPEM('RSA PRIVATE KEY', cipher),
        spkiExp: getRegExpForPEM('PUBLIC KEY'),
        pkcs8Exp: getRegExpForPEM('PRIVATE KEY'),
        pkcs8EncExp: getRegExpForPEM('ENCRYPTED PRIVATE KEY'),
        sec1Exp: getRegExpForPEM('EC PRIVATE KEY'),
        sec1EncExp: (cipher) => getRegExpForPEM('EC PRIVATE KEY', cipher),
        hasOpenSSL,
        get hasOpenSSL3() { return hasOpenSSL(3); },
        get opensslCli() { return false; },
    };
})();

const fixtures = {
    fixturesDir: '/test/fixtures',
    path: (...args) => path.join('/test/fixtures', ...args),
    fileURL: (...args) => url.pathToFileURL(path.join('/test/fixtures', ...args)),
    readSync: (args, enc) => fs.readFileSync(
        Array.isArray(args) ? fixtures.path(...args) : fixtures.path(args), enc),
    readKey: (name, enc) => fs.readFileSync(fixtures.path('keys', name), enc),
    readKeys: (enc, ...names) => names.map((name) => fixtures.readKey(name, enc)),
    get utf8TestText() {
        return fixtures.readSync('utf8_test_text.txt', 'utf8');
    },
    get utf8TestTextPath() {
        return fixtures.path('utf8_test_text.txt');
    },
};

const tmpdir = {
    refresh() {},
    resolve: (...args) => path.resolve(globalThis.__NODE_TEST_TMPDIR__, ...args),
    fileURL: (...args) => url.pathToFileURL(path.resolve(globalThis.__NODE_TEST_TMPDIR__, ...args)),
    hasEnoughSpace: () => true,
    get path() { return globalThis.__NODE_TEST_TMPDIR__; },
    set path(value) { globalThis.__NODE_TEST_TMPDIR__ = path.resolve(String(value)); },
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
    if (name === '../common/tmpdir' || name === '../common/tmpdir.js') return tmpdir;
    if (name === '../common/child_process' || name === '../common/child_process.js') {
        return commonChildProcess;
    }
    if (name === '../common/countdown' || name === '../common/countdown.js') {
        return Countdown;
    }
    if (name === '../common/crypto' || name === '../common/crypto.js') {
        return commonCrypto;
    }
    if (name === '../common/gc' || name === '../common/gc.js'
        || name === '../common/dns' || name === '../common/dns.js'
        || name === '../common/internet' || name === '../common/internet.js'
        || name === '../common/udp' || name === '../common/udp.js') {
        // Capability the sandbox does not provide; the test cannot run.
        common.skip('missing common submodule ' + name);
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
    // The dynamic-import helpers are wrapper parameters rather than prepended
    // statements so a leading 'use strict' directive keeps its force.
    const compiled = Function(
        'exports', 'require', 'module', '__filename', '__dirname',
        '__nodeCompatImport', '__nodeCompatImportWithLoaders', String(source),
    );
    return compiled.call(
        testModule.exports, testModule.exports, nodeRequire, testModule, testPath, testDir,
        globalThis.__NODE_COMPAT_IMPORT__, globalThis.__NODE_COMPAT_IMPORT_WITH_LOADERS__,
    );
};

globalThis.__NODE_TEST_SCHEDULE_REPORT__ = function scheduleReport(sentinel) {
    function scheduleCheck(delay) {
        const timer = globalThis.__NODE_TEST_SETTIMEOUT__(check, delay);
        globalThis.__mcpV8SetTimerResourceTracked(timer, false);
    }
    function check() {
        const active = globalThis.__mcpV8GetActiveResourcesInfo();
        const netHandles = globalThis.__mcpV8NetHandleCount;
        if (active.length > 0 || globalThis.__NODE_TEST_PENDING__ > 0
            || (netHandles && netHandles.refed > 0)) {
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
