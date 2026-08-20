// node:process — minimal process object for the sandboxed runtime.
const startTime = Date.now();
const invocation = globalThis.__mcpV8ProcessConfig || {};
const configuredExecPath = String(invocation.execPath || Deno.core.ops.op_process_exec_path?.() || '/usr/bin/node');
const configuredCwd = String(invocation.cwd || '/');
const setHostExitCode = Deno.core.ops.op_process_set_exit_code;
const terminateHostProcess = Deno.core.ops.op_process_exit;
const hostExitEnabled = invocation.hostExit === true;
function onWarning(warning) {
    const prefix = warning.code ? `[${warning.code}] ` : '';
    console.error(`${prefix}${warning.name}: ${warning.message}`);
}

const eventListeners = new Map([['warning', [onWarning]]]);

function normalizeExitCode(code) {
    const numeric = Number(code);
    return Number.isFinite(numeric) ? Math.trunc(numeric) & 0xff : 0;
}

const process = {
    argv: Array.isArray(invocation.argv) ? [...invocation.argv] : [configuredExecPath, '/main.js'],
    argv0: String(invocation.argv0 || configuredExecPath),
    execArgv: Array.isArray(invocation.execArgv) ? [...invocation.execArgv] : [],
    execPath: configuredExecPath,
    env: invocation.env && typeof invocation.env === 'object' ? { ...invocation.env } : {},
    platform: 'linux',
    arch: 'x64',
    pid: 1,
    ppid: 0,
    version: 'v22.14.0',
    versions: {
        node: '22.14.0',
        v8: '13.0.0',
        modules: '127',
        'mcp-v8': '1.0.0',
    },
    config: {
        variables: {
            node_without_node_options: false,
        },
    },
    title: 'mcp-v8',
    browser: false,

    cwd() { return configuredCwd; },
    chdir() {
        throw new Error('process.chdir is not supported in this runtime');
    },
    umask() { return 0o22; },
    uptime() { return (Date.now() - startTime) / 1000; },
    memoryUsage() {
        return { rss: 0, heapTotal: 0, heapUsed: 0, external: 0, arrayBuffers: 0 };
    },
    getActiveResourcesInfo() {
        const snapshot = globalThis.__mcpV8GetActiveResourcesInfo;
        return typeof snapshot === 'function' ? snapshot() : [];
    },
    hrtime(prev) {
        const now = performance.now();
        const sec = Math.floor(now / 1000);
        const nsec = Math.round((now % 1000) * 1e6);
        if (prev) {
            let ds = sec - prev[0];
            let dn = nsec - prev[1];
            if (dn < 0) { ds -= 1; dn += 1e9; }
            return [ds, dn];
        }
        return [sec, nsec];
    },
    nextTick(callback, ...args) {
        if (typeof callback !== 'function') {
            throw new TypeError('Callback must be a function');
        }
        queueMicrotask(() => callback(...args));
    },
    emitWarning(warning, type = 'Warning', code) {
        const normalized = warning instanceof Error ? warning : new Error(String(warning));
        if (!(warning instanceof Error)) normalized.name = String(type || 'Warning');
        if (!(warning instanceof Error) && code !== undefined) normalized.code = String(code);
        process.nextTick(() => process.emit('warning', normalized));
    },
    exit(code) {
        const normalized = code === undefined ? process.exitCode : normalizeExitCode(code);
        process.exitCode = normalized;
        if (!hostExitEnabled) {
            const error = new Error('process.exit is not supported in this runtime');
            error.code = 'ERR_UNSUPPORTED_OPERATION';
            throw error;
        }
        terminateHostProcess(normalized);
    },
    abort() { this.exit(134); },
    kill() {
        throw new Error('process.kill is not supported in this runtime');
    },
    stdout: {
        write(chunk) { console.log(typeof chunk === 'string' ? chunk.replace(/\n$/, '') : chunk); return true; },
        isTTY: false,
    },
    stderr: {
        write(chunk) { console.error(typeof chunk === 'string' ? chunk.replace(/\n$/, '') : chunk); return true; },
        isTTY: false,
    },
    stdin: null,
    on(eventName, listener) {
        return process.addListener(eventName, listener);
    },
    once(eventName, listener) {
        if (typeof listener !== 'function') throw new TypeError('The listener argument must be a function');
        const wrapped = (...args) => {
            process.removeListener(eventName, wrapped);
            return Reflect.apply(listener, process, args);
        };
        wrapped.listener = listener;
        return process.addListener(eventName, wrapped);
    },
    off(eventName, listener) {
        return process.removeListener(eventName, listener);
    },
    removeListener(eventName, listener) {
        const listeners = eventListeners.get(eventName);
        if (!listeners) return process;
        const index = listeners.findLastIndex((candidate) =>
            candidate === listener || candidate.listener === listener);
        if (index >= 0) listeners.splice(index, 1);
        if (listeners.length === 0) eventListeners.delete(eventName);
        return process;
    },
    removeAllListeners(eventName) {
        if (eventName === undefined) eventListeners.clear();
        else eventListeners.delete(eventName);
        return process;
    },
    listeners(eventName) {
        return (eventListeners.get(eventName) || []).map((listener) => listener.listener || listener);
    },
    emit(eventName, ...args) {
        const listeners = eventListeners.get(eventName);
        if (!listeners || listeners.length === 0) return false;
        for (const listener of [...listeners]) Reflect.apply(listener, process, args);
        return true;
    },
    addListener(eventName, listener) {
        if (typeof listener !== 'function') throw new TypeError('The listener argument must be a function');
        const listeners = eventListeners.get(eventName);
        if (listeners) listeners.push(listener);
        else eventListeners.set(eventName, [listener]);
        return process;
    },
    prependListener(eventName, listener) {
        if (typeof listener !== 'function') throw new TypeError('The listener argument must be a function');
        const listeners = eventListeners.get(eventName);
        if (listeners) listeners.unshift(listener);
        else eventListeners.set(eventName, [listener]);
        return process;
    },
    setMaxListeners() { return process; },
    getMaxListeners() { return 10; },
};

Object.defineProperty(process, Symbol.toStringTag, {
    value: 'process',
    writable: true,
    enumerable: false,
    configurable: false,
});

function diagnosticStack(error) {
    if (!(error instanceof Error)) return { message: '', stack: [], errorProperties: {} };
    const lines = String(error.stack || error).split('\n');
    return {
        message: lines.shift() || `${error.name}: ${error.message}`,
        stack: lines.map((line) => line.trim()),
        errorProperties: {},
    };
}

const diagnosticReport = {
    compact: false,
    directory: '',
    excludeEnv: false,
    excludeNetwork: false,
    filename: '',
    reportOnFatalError: false,
    reportOnSignal: false,
    reportOnUncaughtException: false,
    signal: 'SIGUSR2',

    getReport(error) {
        const now = new Date();
        return {
            header: {
                reportVersion: 5,
                event: 'JavaScript API',
                trigger: 'GetReport',
                filename: null,
                dumpEventTime: now.toISOString(),
                dumpEventTimeStamp: String(now.getTime()),
                processId: process.pid,
                threadId: 0,
                cwd: process.cwd(),
                commandLine: [...process.argv],
                nodejsVersion: process.version,
                wordSize: '64 bit',
                arch: process.arch,
                platform: process.platform,
                componentVersions: { ...process.versions },
                release: { name: 'node' },
                cpus: [],
                networkInterfaces: this.excludeNetwork || typeof Deno.networkInterfaces !== 'function'
                    ? []
                    : Deno.networkInterfaces(),
            },
            javascriptStack: diagnosticStack(error),
            javascriptHeap: {},
            nativeStack: [],
            resourceUsage: {},
            uvthreadResourceUsage: {},
            libuv: [],
            workers: [],
            environmentVariables: this.excludeEnv ? {} : { ...process.env },
            userLimits: {},
            sharedObjects: [],
        };
    },

    writeReport(filename, error) {
        if (filename instanceof Error && error === undefined) {
            error = filename;
            filename = undefined;
        }
        let target = filename === undefined ? this.filename : String(filename);
        if (!target) {
            const timestamp = new Date().toISOString().replace(/\D/g, '').slice(0, 14);
            target = `report.${timestamp}.${process.pid}.0.001.json`;
        }
        if (this.directory && !target.startsWith('/')) {
            target = `${this.directory.replace(/\/+$/, '')}/${target}`;
        }
        Deno.writeTextFileSync(
            target,
            JSON.stringify(this.getReport(error), null, this.compact ? 0 : 2),
        );
        return target;
    },
};
process.report = diagnosticReport;

let exitCode = 0;
Object.defineProperty(process, 'exitCode', {
    enumerable: true,
    configurable: true,
    get() { return exitCode; },
    set(code) {
        exitCode = normalizeExitCode(code);
        setHostExitCode(exitCode);
    },
});

process.hrtime.bigint = function bigint() {
    return BigInt(Math.round(performance.now() * 1e6));
};

export default process;
export const {
    argv, argv0, execArgv, execPath, env, platform, arch, pid, ppid,
    version, versions, config, title, browser, nextTick, cwd, chdir, umask, uptime,
    memoryUsage, hrtime, getActiveResourcesInfo, emitWarning, exit, abort,
    kill, stdout, stderr, stdin, report,
} = process;
