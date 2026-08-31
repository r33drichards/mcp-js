// node:net — address helpers plus Socket/Server. Raw TCP is deliberately
// absent from the default sandbox: outbound networking goes through the
// policy-gated fetch / WebSocket / node:http2 capabilities, where
// per-request policy and server-side header injection hold (see
// docs/node-http2-grpc-plan.md). When the host enables the loopback-only
// TCP capability (Node-compatibility harnesses), `__mcpV8NetOps` is bound
// before hardening and Socket/Server become real — pinned to 127.0.0.1 by
// the Rust side. Without it they stay inert: constructing and configuring
// works, moving bytes emits an error explaining the capability model.

import { EventEmitter } from 'node:events';
import { Duplex } from 'node:stream';
import { Buffer } from 'node:buffer';

const ops = globalThis.__mcpV8NetOps;

const V4_SEG = '(25[0-5]|2[0-4][0-9]|1[0-9][0-9]|[1-9]?[0-9])';
const V4_RE = new RegExp(`^${V4_SEG}\\.${V4_SEG}\\.${V4_SEG}\\.${V4_SEG}$`);

export function isIPv4(input) {
    return typeof input === 'string' && V4_RE.test(input);
}

export function isIPv6(input) {
    if (typeof input !== 'string' || input.length === 0) return false;
    if (input.includes('.')) {
        // Mixed notation ::ffff:1.2.3.4
        const lastColon = input.lastIndexOf(':');
        if (lastColon === -1) return false;
        if (!isIPv4(input.slice(lastColon + 1))) return false;
        input = input.slice(0, lastColon + 1) + '0:0';
    }
    const parts = input.split('::');
    if (parts.length > 2) return false;
    const groups = (side) => (side === '' ? [] : side.split(':'));
    const head = groups(parts[0]);
    const tail = parts.length === 2 ? groups(parts[1]) : [];
    if (head.some((g) => !/^[0-9a-fA-F]{1,4}$/.test(g))) return false;
    if (tail.some((g) => !/^[0-9a-fA-F]{1,4}$/.test(g))) return false;
    const total = head.length + tail.length;
    if (parts.length === 2) return total < 8;
    return total === 8;
}

export function isIP(input) {
    if (isIPv4(input)) return 4;
    if (isIPv6(input)) return 6;
    return 0;
}

let defaultAutoSelectFamilyAttemptTimeout = 2500;

export function getDefaultAutoSelectFamilyAttemptTimeout() {
    return defaultAutoSelectFamilyAttemptTimeout;
}

export function setDefaultAutoSelectFamilyAttemptTimeout(value) {
    if (typeof value !== 'number') {
        const error = new TypeError(
            `The "value" argument must be of type number. Received type ${typeof value}`);
        error.code = 'ERR_INVALID_ARG_TYPE';
        throw error;
    }
    if (!Number.isInteger(value)) {
        const error = new RangeError(
            `The value of "value" is out of range. It must be an integer. Received ${value}`);
        error.code = 'ERR_OUT_OF_RANGE';
        throw error;
    }
    if (value < 1 || value > 0x7fffffff) {
        const error = new RangeError(
            `The value of "value" is out of range. It must be >= 1 && <= 2147483647. Received ${value}`);
        error.code = 'ERR_OUT_OF_RANGE';
        throw error;
    }
    defaultAutoSelectFamilyAttemptTimeout = Math.max(10, value);
}

function opError(message) {
    const match = /\((E[A-Z]+)\)/.exec(message);
    const err = new Error(message);
    if (match) err.code = match[1];
    else if (message.includes('ECONNREFUSED')) err.code = 'ECONNREFUSED';
    return err;
}

function normalizeConnectArgs(args) {
    let options = {};
    let cb;
    if (typeof args[0] === 'object' && args[0] !== null) {
        options = args[0];
        cb = args[1];
    } else if (typeof args[0] === 'number' || typeof args[0] === 'string' && /^\d+$/.test(args[0])) {
        options = { port: Number(args[0]) };
        if (typeof args[1] === 'string') {
            options.host = args[1];
            cb = args[2];
        } else {
            cb = args[1];
        }
    } else {
        cb = args[1];
    }
    return [options, typeof cb === 'function' ? cb : undefined];
}

export class Socket extends Duplex {
    constructor(_options) {
        super({});
        this.connecting = false;
        this.pending = true;
        this.destroyed = false;
        this.readable = false;
        this.writable = false;
        this.remoteAddress = undefined;
        this.remotePort = undefined;
        this.remoteFamily = undefined;
        this.localAddress = undefined;
        this.localPort = undefined;
        this.bytesRead = 0;
        this.bytesWritten = 0;
        this._rid = null;
        this._endSent = false;
        this._timeoutMs = 0;
        this._timeoutTimer = null;
        this._server = null;
        this.allowHalfOpen = Boolean(_options && _options.allowHalfOpen);
        // Fires on both the destroy path and graceful end+finish teardown.
        this.once('close', () => this._teardown());
    }

    _teardown() {
        this._clearTimeout();
        const rid = this._rid;
        this._rid = null;
        if (rid !== null && ops) ops.closeStream(rid);
        if (this._server) {
            const server = this._server;
            this._server = null;
            server._socketClosed(this);
        }
    }

    _adopt(info, server) {
        this._rid = info.rid;
        this.pending = false;
        this.connecting = false;
        this.readable = true;
        this.writable = true;
        this.remoteAddress = info.remoteAddress;
        this.remotePort = info.remotePort;
        this.remoteFamily = info.remoteFamily;
        this.localAddress = info.localAddress;
        this.localPort = info.localPort;
        this._server = server || null;
        this._startReading();
    }

    connect(...args) {
        if (!ops) {
            Promise.resolve().then(() => {
                this.emit('error', new Error(
                    'net.Socket.connect is not supported in this runtime: use the ' +
                    'policy-gated fetch / WebSocket / node:http2 capabilities instead'));
            });
            return this;
        }
        const [options, cb] = normalizeConnectArgs(args);
        const port = Number(options.port);
        const host = options.host || options.hostname || '127.0.0.1';
        this.connecting = true;
        if (cb) this.once('connect', cb);
        ops.connect(String(host), port >>> 0).then((json) => {
            if (this.destroyed) {
                ops.closeStream(JSON.parse(json).rid);
                return;
            }
            this._adopt(JSON.parse(json));
            this.emit('connect');
            this.emit('ready');
        }, (error) => {
            this.connecting = false;
            this.destroy(opError(String(error && error.message || error)));
        });
        return this;
    }

    async _startReading() {
        const rid = this._rid;
        while (this._rid === rid && !this.destroyed) {
            let result;
            try {
                result = JSON.parse(await ops.read(rid));
            } catch {
                break;
            }
            if (this._rid !== rid || this.destroyed) break;
            this._touchTimeout();
            if (result.data !== undefined) {
                const chunk = Buffer.from(result.data, 'base64');
                this.bytesRead += chunk.length;
                this.push(chunk);
            } else if (result.eof) {
                this.readable = false;
                this.push(null);
                if (!this.allowHalfOpen && !this._endSent) this.end();
                break;
            } else if (result.closed) {
                break;
            } else {
                this.destroy(opError(result.error || 'read error'));
                break;
            }
        }
    }

    _write(chunk, encoding, callback) {
        if (this._rid === null) {
            // Not connected yet: queue behind the connect.
            this.once('connect', () => this._write(chunk, encoding, callback));
            if (!this.connecting && !ops) {
                callback(new Error('net.Socket is not connected'));
            }
            return;
        }
        const buf = Buffer.isBuffer(chunk) ? chunk : Buffer.from(String(chunk), encoding || 'utf8');
        this.bytesWritten += buf.length;
        this._touchTimeout();
        ops.write(this._rid, buf.toString('base64')).then(
            () => callback(),
            (error) => callback(opError(String(error && error.message || error))));
    }

    _final(callback) {
        this._endSent = true;
        if (this._rid === null) { callback(); return; }
        ops.shutdown(this._rid).then(() => callback(), () => callback());
    }

    _destroy(err, callback) {
        this.readable = false;
        this.writable = false;
        this._teardown();
        callback(err || null);
    }

    address() {
        if (this.localAddress === undefined) return {};
        return {
            address: this.localAddress,
            port: this.localPort,
            family: this.remoteFamily || 'IPv4',
        };
    }

    setTimeout(ms, callback) {
        this._timeoutMs = ms;
        if (callback) {
            if (ms === 0) this.removeListener('timeout', callback);
            else this.once('timeout', callback);
        }
        this._touchTimeout();
        return this;
    }

    _touchTimeout() {
        this._clearTimeout();
        if (this._timeoutMs > 0) {
            this._timeoutTimer = setTimeout(() => {
                this.emit('timeout');
            }, this._timeoutMs);
        }
    }

    _clearTimeout() {
        if (this._timeoutTimer !== null) {
            clearTimeout(this._timeoutTimer);
            this._timeoutTimer = null;
        }
    }

    setNoDelay() { return this; }
    setKeepAlive() { return this; }
    ref() { return this; }
    unref() { return this; }
}

export class Server extends EventEmitter {
    constructor(options, connectionListener) {
        super();
        if (typeof options === 'function') {
            connectionListener = options;
            options = {};
        }
        this.listening = false;
        this.maxConnections = undefined;
        this._rid = null;
        this._address = null;
        this._connections = new Set();
        this._closing = false;
        this._options = options || {};
        if (typeof connectionListener === 'function') {
            this.on('connection', connectionListener);
        }
    }

    listen(...args) {
        if (!ops) {
            throw new Error('net.createServer is not supported in this runtime');
        }
        let options = {};
        let cb;
        if (typeof args[0] === 'object' && args[0] !== null) {
            options = args[0];
            cb = args[1];
        } else {
            options.port = args[0];
            let i = 1;
            if (typeof args[i] === 'string') options.host = args[i++];
            if (typeof args[i] === 'number') i++; // backlog
            cb = args[i];
        }
        if (typeof cb === 'function') this.once('listening', cb);
        const port = Number(options.port) || 0;
        const host = options.host || '';
        let json;
        try {
            json = ops.listen(String(host), port >>> 0);
        } catch (error) {
            const err = opError(String(error && error.message || error));
            Promise.resolve().then(() => this.emit('error', err));
            return this;
        }
        const info = JSON.parse(json);
        this._rid = info.rid;
        this._address = { address: info.address, port: info.port, family: info.family };
        this.listening = true;
        Promise.resolve().then(() => this.emit('listening'));
        this._acceptLoop(info.rid);
        return this;
    }

    async _acceptLoop(rid) {
        while (this._rid === rid) {
            let result;
            try {
                result = JSON.parse(await ops.accept(rid));
            } catch {
                break;
            }
            if (this._rid !== rid) {
                if (result.rid !== undefined) ops.closeStream(result.rid);
                break;
            }
            if (result.closed) break;
            const socket = new Socket();
            socket._adopt(result, this);
            this._connections.add(socket);
            this.emit('connection', socket);
        }
        this._maybeEmitClose();
    }

    _socketClosed(socket) {
        this._connections.delete(socket);
        this._maybeEmitClose();
    }

    _maybeEmitClose() {
        if (this._closing && this._rid === null && this._connections.size === 0) {
            this._closing = false;
            Promise.resolve().then(() => this.emit('close'));
        }
    }

    close(cb) {
        if (typeof cb === 'function') {
            if (!this.listening && this._rid === null) {
                const err = new Error('Server is not running.');
                err.code = 'ERR_SERVER_NOT_RUNNING';
                Promise.resolve().then(() => cb(err));
                return this;
            }
            this.once('close', cb);
        }
        this._closing = true;
        this.listening = false;
        const rid = this._rid;
        this._rid = null;
        if (rid !== null && ops) ops.closeListener(rid);
        this._maybeEmitClose();
        return this;
    }

    address() {
        return this._address;
    }

    getConnections(cb) {
        Promise.resolve().then(() => cb(null, this._connections.size));
        return this;
    }

    ref() { return this; }
    unref() { return this; }
}

export function connect(...args) {
    const socket = new Socket(typeof args[0] === 'object' ? args[0] : undefined);
    return socket.connect(...args);
}

export const createConnection = connect;

export function createServer(options, connectionListener) {
    if (!ops) {
        throw new Error('net.createServer is not supported in this runtime');
    }
    return new Server(options, connectionListener);
}

export default {
    isIP,
    isIPv4,
    isIPv6,
    getDefaultAutoSelectFamilyAttemptTimeout,
    setDefaultAutoSelectFamilyAttemptTimeout,
    Socket,
    Server,
    connect,
    createConnection,
    createServer,
};
