// node:process — minimal process object for the sandboxed runtime.
// No real OS process exists: env is empty (host env is never exposed),
// exit() throws, and cwd is fixed at '/'.
const startTime = Date.now();

const process = {
    argv: ['node', '/main.js'],
    argv0: 'node',
    execArgv: [],
    execPath: '/usr/bin/node',
    env: {},
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
    title: 'mcp-v8',
    browser: false,

    cwd() { return '/'; },
    chdir() {
        throw new Error('process.chdir is not supported in this runtime');
    },
    umask() { return 0o22; },
    uptime() { return (Date.now() - startTime) / 1000; },
    memoryUsage() {
        return { rss: 0, heapTotal: 0, heapUsed: 0, external: 0, arrayBuffers: 0 };
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
    emitWarning(warning) {
        console.warn(warning instanceof Error ? warning.message : String(warning));
    },
    exit(code) {
        const err = new Error(
            'process.exit(' + (code === undefined ? '' : code) + ') is not supported in this runtime');
        err.code = 'ERR_UNSUPPORTED_OPERATION';
        throw err;
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
    on() { return this; },
    once() { return this; },
    off() { return this; },
    removeListener() { return this; },
    removeAllListeners() { return this; },
    listeners() { return []; },
    emit() { return false; },
    addListener() { return this; },
    prependListener() { return this; },
    setMaxListeners() { return this; },
    getMaxListeners() { return 10; },
};

process.hrtime.bigint = function bigint() {
    return BigInt(Math.round(performance.now() * 1e6));
};

export default process;
export const { argv, env, platform, arch, version, versions, nextTick, cwd, hrtime } = process;
