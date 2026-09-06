// node:cluster — primary/worker process model over the loopback-only TCP
// capability. fork() re-executes the current entrypoint through the
// self-hosted Node-compatibility CLI (like worker_threads does) and the
// IPC channel is a newline-delimited JSON stream over a 127.0.0.1 socket
// the primary listens on. Shared server ports are NOT implemented — each
// worker binds its own — but lifecycle (fork/online/listening/message/
// disconnect/exit), worker.send both ways, and kill/destroy are real.

import { EventEmitter } from 'node:events';
import net from 'node:net';
import process from 'node:process';
import { Buffer } from 'node:buffer';

function hostPath(filename) {
    const value = String(filename);
    const corpus = globalThis.__NODE_TEST_CORPUS_HOST__;
    return corpus && value.startsWith('/test/') ? corpus + value : value;
}

class Cluster extends EventEmitter {}
const cluster = new Cluster();

const uniqueId = process.env.NODE_UNIQUE_ID;
const ipcPort = Number(process.env.MCP_V8_CLUSTER_IPC_PORT || 0);

cluster.isWorker = Boolean(uniqueId);
cluster.isPrimary = !cluster.isWorker;
cluster.isMaster = cluster.isPrimary;
cluster.SCHED_NONE = 1;
cluster.SCHED_RR = 2;
cluster.schedulingPolicy = cluster.SCHED_RR;
cluster.settings = {};
cluster.workers = cluster.isPrimary ? {} : undefined;
cluster.worker = undefined;

cluster.setupPrimary = function setupPrimary(options) {
    cluster.settings = { ...cluster.settings, ...options };
    cluster.emit('setup', cluster.settings);
    return cluster.settings;
};
cluster.setupMaster = cluster.setupPrimary;
cluster.disconnect = function disconnect(callback) {
    const workers = Object.values(cluster.workers || {});
    let remaining = workers.length;
    const done = () => {
        if (--remaining <= 0 && typeof callback === 'function') callback();
    };
    if (remaining === 0) {
        if (typeof callback === 'function') Promise.resolve().then(callback);
        return;
    }
    for (const worker of workers) {
        worker.once('exit', done);
        worker.disconnect();
    }
};

// ── framed JSON over a socket ───────────────────────────────────────────

function attachJsonStream(socket, onMessage) {
    let pending = Buffer.alloc(0);
    socket.on('data', (chunk) => {
        pending = pending.length === 0 ? chunk : Buffer.concat([pending, chunk]);
        for (;;) {
            const newline = pending.indexOf(0x0a);
            if (newline === -1) return;
            const line = pending.subarray(0, newline).toString('utf8');
            pending = pending.subarray(newline + 1);
            if (!line.trim()) continue;
            let value;
            try {
                value = JSON.parse(line);
            } catch {
                continue;
            }
            onMessage(value);
        }
    });
}

function writeJson(socket, value) {
    if (!socket || socket.destroyed) return false;
    try {
        socket.write(JSON.stringify(value) + '\n');
        return true;
    } catch {
        return false;
    }
}

// ── worker handle (both sides) ──────────────────────────────────────────

class Worker extends EventEmitter {
    constructor(options = {}) {
        super();
        this.id = options.id;
        this.process = options.process || null;
        this.state = options.state || 'none';
        this.exitedAfterDisconnect = undefined;
    }

    send(message, _handle, _options, callback) {
        if (typeof _handle === 'function') callback = _handle;
        else if (typeof _options === 'function') callback = _options;
        const ok = this._channel
            ? writeJson(this._channel, { t: 'user', message })
            : false;
        if (typeof callback === 'function') {
            Promise.resolve().then(() => callback(ok ? null : new Error('channel closed')));
        }
        return ok;
    }

    isConnected() {
        return Boolean(this._channel && !this._channel.destroyed);
    }

    isDead() {
        return this.state === 'dead';
    }

    disconnect() {
        this.exitedAfterDisconnect = true;
        if (this._channel && !this._channel.destroyed) {
            writeJson(this._channel, { t: 'disconnect' });
            this._channel.end();
        }
        return this;
    }

    kill(signal = 'SIGTERM') {
        this.destroy(signal);
    }

    destroy(signal = 'SIGTERM') {
        this.exitedAfterDisconnect = true;
        this._killSignal = signal;
        if (this._terminate) this._terminate(signal);
        else if (this._channel && !this._channel.destroyed) {
            writeJson(this._channel, { t: 'kill', signal });
            this._channel.end();
        }
    }
}

// ── primary side ────────────────────────────────────────────────────────

let ipcServer = null;
let ipcServerPort = 0;
let ipcReady = null;
let nextWorkerId = 1;
const awaitingChannel = new Map();

function ensureIpcServer() {
    if (ipcReady) return ipcReady;
    ipcServer = net.createServer((socket) => {
        let claimed = null;
        attachJsonStream(socket, (message) => {
            if (!claimed) {
                if (message.t !== 'hello') return;
                claimed = awaitingChannel.get(String(message.id));
                if (!claimed) {
                    socket.destroy();
                    return;
                }
                awaitingChannel.delete(String(message.id));
                claimed._channel = socket;
                socket.on('close', () => {
                    if (claimed._channel === socket) claimed._channel = null;
                    claimed.emit('disconnect');
                    cluster.emit('disconnect', claimed);
                });
                return;
            }
            const worker = claimed;
            if (message.t === 'online') {
                worker.state = 'online';
                worker.emit('online');
                cluster.emit('online', worker);
            } else if (message.t === 'listening') {
                worker.state = 'listening';
                worker.emit('listening', message.address);
                cluster.emit('listening', worker, message.address);
            } else if (message.t === 'user') {
                worker.emit('message', message.message, undefined);
                cluster.emit('message', worker, message.message, undefined);
            }
        });
    });
    ipcServer.unref();
    ipcReady = new Promise((resolve, reject) => {
        ipcServer.on('error', reject);
        ipcServer.listen(0, '127.0.0.1', () => {
            ipcServerPort = ipcServer.address().port;
            resolve(ipcServerPort);
        });
    });
    return ipcReady;
}

cluster.fork = function fork(env) {
    if (!cluster.isPrimary) {
        throw new Error('cluster.fork() can only be called from the primary');
    }
    const id = nextWorkerId++;
    const worker = new Worker({ id, state: 'none' });
    cluster.workers[id] = worker;
    awaitingChannel.set(String(id), worker);

    const entry = cluster.settings.exec
        ? hostPath(cluster.settings.exec)
        : hostPath(process.argv[1] || '');
    const executable = globalThis.__NODE_TEST_EXEC_PATH__
        || process.execPath || 'node';
    const controlPath = `/tmp/mcp-v8-cluster-${Date.now()}-${Math.random()}.pid`;

    ensureIpcServer().then((port) => {
        const environment = {
            NODE_UNIQUE_ID: String(id),
            MCP_V8_CLUSTER_IPC_PORT: String(port),
        };
        if (env && typeof env === 'object') {
            for (const key of Object.keys(env)) {
                environment[key] = String(env[key]);
            }
        }
        const args = Array.isArray(cluster.settings.args) ? cluster.settings.args : [];
        const command = new Deno.Command('/bin/sh', {
            args: [
                '-c',
                'printf %s "$$" > "$1"; shift; exec "$@"',
                'mcp-v8-cluster-worker',
                controlPath,
                executable,
                '--node-compat-cli',
                entry,
                ...args.map(String),
            ],
            env: environment,
        });
        worker._terminate = async (signal) => {
            const kill = new Deno.Command('/bin/sh', {
                args: [
                    '-c',
                    'i=0; while [ ! -s "$1" ] && [ "$i" -lt 100 ]; do sleep 0.01; i=$((i+1)); done; ' +
                    '[ ! -s "$1" ] || kill -s "${2#SIG}" "$(cat "$1")" 2>/dev/null || true',
                    'mcp-v8-cluster-kill',
                    controlPath,
                    String(signal || 'SIGTERM'),
                ],
            });
            await kill.output();
        };
        command.output().then((output) => {
            worker.state = 'dead';
            delete cluster.workers[id];
            awaitingChannel.delete(String(id));
            const signal = worker._killSignal && !output.success
                ? worker._killSignal : null;
            const code = signal ? null : output.code;
            worker.emit('exit', code, signal);
            cluster.emit('exit', worker, code, signal);
        }, (error) => {
            worker.state = 'dead';
            delete cluster.workers[id];
            worker.emit('error', error);
            worker.emit('exit', -1, null);
            cluster.emit('exit', worker, -1, null);
        });
    }, (error) => {
        Promise.resolve().then(() => worker.emit('error', error));
    });

    Promise.resolve().then(() => cluster.emit('fork', worker));
    return worker;
};

// ── worker side ─────────────────────────────────────────────────────────

if (cluster.isWorker) {
    const worker = new Worker({ id: Number(uniqueId), state: 'online', process });
    cluster.worker = worker;
    if (ipcPort > 0) {
        const channel = net.connect(ipcPort, '127.0.0.1', () => {
            writeJson(channel, { t: 'hello', id: uniqueId });
            writeJson(channel, { t: 'online' });
        });
        channel.unref();
        worker._channel = channel;
        attachJsonStream(channel, (message) => {
            if (message.t === 'user') {
                worker.emit('message', message.message, undefined);
                process.emit('message', message.message, undefined);
            } else if (message.t === 'disconnect') {
                channel.end();
            } else if (message.t === 'kill') {
                channel.destroy();
                process.exit(0);
            }
        });
        channel.on('close', () => {
            if (worker._channel === channel) worker._channel = null;
            process.connected = false;
            worker.emit('disconnect');
            cluster.emit('disconnect', worker);
            process.emit('disconnect');
        });
        channel.on('error', () => {});
        // Route process.send through the cluster channel, like Node's
        // worker IPC.
        process.connected = true;
        process.send = (message, handle, options, callback) =>
            worker.send(message, handle, options, callback);
        process.disconnect = () => worker.disconnect();
        // net/dgram report successful binds to the primary.
        try {
            Object.defineProperty(globalThis, '__mcpV8ClusterOnListening', {
                value: (address) => writeJson(channel, { t: 'listening', address }),
                writable: false, enumerable: false, configurable: false,
            });
        } catch { /* already defined */ }
    }
}

export const {
    isWorker, isPrimary, isMaster, SCHED_NONE, SCHED_RR,
} = cluster;
export const fork = (...args) => cluster.fork(...args);
export const disconnect = (...args) => cluster.disconnect(...args);
export const setupPrimary = cluster.setupPrimary;
export const setupMaster = cluster.setupMaster;
export { Worker };
export default cluster;
