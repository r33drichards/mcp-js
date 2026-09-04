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

// Shared count of open, refed net/dgram handles. The corpus harness's
// end-of-test check consults it (like Node's active-handle accounting) so
// reports wait for sockets to close or unref instead of racing them.
function netHandleRegistry() {
    if (!globalThis.__mcpV8NetHandleCount) {
        try {
            Object.defineProperty(globalThis, '__mcpV8NetHandleCount', {
                value: { refed: 0 },
                writable: false, enumerable: false, configurable: false,
            });
        } catch {
            return { refed: 0 };
        }
    }
    return globalThis.__mcpV8NetHandleCount;
}
const handleRegistry = netHandleRegistry();

// Mixin: each handle contributes 1 while open and refed.
function syncHandle(self, open) {
    self._handleOpen = open;
    const contribution = (self._handleOpen && !self._unrefed) ? 1 : 0;
    handleRegistry.refed += contribution - (self._handleContrib || 0);
    self._handleContrib = contribution;
}

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

function receivedRepr(value) {
    if (value === undefined) return 'undefined';
    if (value === null) return 'null';
    const type = typeof value;
    if (type === 'string') return `type string ('${value}')`;
    if (type === 'object') {
        const name = value.constructor && value.constructor.name;
        return `an instance of ${name || 'Object'}`;
    }
    return `type ${type} (${String(value)})`;
}

// Node's validatePort: wrong TYPE is ERR_INVALID_ARG_TYPE, a number/string
// with a bad VALUE (including '' and 65536+) is ERR_SOCKET_BAD_PORT; 0 is
// valid here and fails later at the transport.
export function validatePort(port, name = 'options.port') {
    if (typeof port !== 'number' && typeof port !== 'string') {
        const err = new TypeError(
            `The "${name}" property must be of type number or string. Received ${receivedRepr(port)}`);
        err.code = 'ERR_INVALID_ARG_TYPE';
        throw err;
    }
    if ((typeof port === 'string' && port.trim().length === 0)
        || +port !== (+port >>> 0) || +port > 0xFFFF) {
        const err = new RangeError(
            `${name} should be >= 0 and < 65536. Received ${receivedRepr(port)}.`);
        err.code = 'ERR_SOCKET_BAD_PORT';
        throw err;
    }
    return +port;
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
    } else {
        // Anything else lands in port — validatePort rejects bad values.
        options = { port: args[0] };
        if (typeof args[1] === 'string') {
            options.host = args[1];
            cb = args[2];
        } else {
            cb = args[1];
        }
    }
    return [options, typeof cb === 'function' ? cb : undefined];
}

// Node's net/http constructors predate class syntax and are callable
// without `new` (`net.Server(fn)` works); a Proxy apply trap preserves
// that while keeping instanceof and prototype identity.
function callable(Cls) {
    return new Proxy(Cls, {
        apply: (target, _thisArg, args) => new target(...args),
    });
}

class SocketImpl extends Duplex {
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
        syncHandle(this, false);
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
        syncHandle(this, true);
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
        const port = validatePort(options.port, 'options.port');
        const host = options.host || options.hostname || '127.0.0.1';
        if (host !== 'localhost' && !isIPv4(host) && !isIPv6(host)) {
            // The sandbox has no resolver; an unknown hostname surfaces the
            // way Node reports a failed lookup.
            const err = new Error(`getaddrinfo ENOTFOUND ${host}`);
            err.code = 'ENOTFOUND';
            err.errno = -3008;
            err.syscall = 'getaddrinfo';
            err.hostname = host;
            Promise.resolve().then(() => this.destroy(err));
            return this;
        }
        if (options.blockList && typeof options.blockList.check === 'function') {
            const ip = host === 'localhost' ? '127.0.0.1' : host;
            const family = isIPv6(ip) ? 'ipv6' : 'ipv4';
            if (options.blockList.check(ip, family)) {
                const err = new Error(`connect ERR_IP_BLOCKED ${ip}:${port}`);
                err.code = 'ERR_IP_BLOCKED';
                Promise.resolve().then(() => this.destroy(err));
                return this;
            }
        }
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
                this._readPromise = ops.read(rid);
                if (this._unrefed && ops.unrefOpPromise) {
                    ops.unrefOpPromise(this._readPromise);
                }
                result = JSON.parse(await this._readPromise);
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
        this._timeoutFired = false;
        if (callback) {
            if (ms === 0) this.removeListener('timeout', callback);
            else this.once('timeout', callback);
        }
        this._touchTimeout();
        return this;
    }

    _touchTimeout() {
        this._clearTimeout();
        // Once fired, activity does not re-arm the idle timer — only an
        // explicit setTimeout() call does (matches observable Node behavior).
        if (this._timeoutMs > 0 && !this._timeoutFired) {
            this._timeoutTimer = setTimeout(() => {
                this._timeoutFired = true;
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

    resetAndDestroy() {
        // Approximation: a hard close (the loopback transport has no RST
        // control), which still tears the connection down immediately.
        return this.destroy();
    }

    setNoDelay() { return this; }
    setKeepAlive() { return this; }
    ref() {
        this._unrefed = false;
        syncHandle(this, this._handleOpen);
        if (this._readPromise && ops.refOpPromise) ops.refOpPromise(this._readPromise);
        return this;
    }

    unref() {
        this._unrefed = true;
        syncHandle(this, this._handleOpen);
        if (this._readPromise && ops.unrefOpPromise) ops.unrefOpPromise(this._readPromise);
        return this;
    }
}

class ServerImpl extends EventEmitter {
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
        } else {
            options.port = args[0];
            if (typeof args[1] === 'string') options.host = args[1];
        }
        // The callback is whatever trailing function was passed, however
        // many host/backlog slots (possibly undefined) sit before it.
        const last = args[args.length - 1];
        if (typeof last === 'function') cb = last;
        if (typeof cb === 'function') this.once('listening', cb);
        const port = options.port === undefined || options.port === null
            ? 0 : validatePort(options.port);
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
        syncHandle(this, true);
        if (globalThis.__mcpV8ClusterOnListening) {
            globalThis.__mcpV8ClusterOnListening({
                address: info.address,
                port: info.port,
                addressType: info.family === 'IPv6' ? 6 : 4,
                fd: -1,
            });
        }
        Promise.resolve().then(() => this.emit('listening'));
        this._acceptLoop(info.rid);
        return this;
    }

    async _acceptLoop(rid) {
        while (this._rid === rid) {
            let result;
            try {
                this._acceptPromise = ops.accept(rid);
                if (this._unrefed && ops.unrefOpPromise) {
                    ops.unrefOpPromise(this._acceptPromise);
                }
                result = JSON.parse(await this._acceptPromise);
            } catch {
                break;
            }
            if (this._rid !== rid) {
                if (result.rid !== undefined) ops.closeStream(result.rid);
                break;
            }
            if (result.closed) break;
            const blockList = this._options.blockList;
            if (blockList && typeof blockList.check === 'function'
                && blockList.check(
                    result.remoteAddress,
                    result.remoteFamily === 'IPv6' ? 'ipv6' : 'ipv4')) {
                ops.closeStream(result.rid);
                this.emit('drop', {
                    localAddress: result.localAddress,
                    localPort: result.localPort,
                    localFamily: result.localFamily,
                    remoteAddress: result.remoteAddress,
                    remotePort: result.remotePort,
                    remoteFamily: result.remoteFamily,
                });
                continue;
            }
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
        if (this._rid === null) syncHandle(this, false);
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

    ref() {
        this._unrefed = false;
        syncHandle(this, this._handleOpen);
        if (this._acceptPromise && ops.refOpPromise) ops.refOpPromise(this._acceptPromise);
        return this;
    }

    unref() {
        this._unrefed = true;
        syncHandle(this, this._handleOpen);
        if (this._acceptPromise && ops.unrefOpPromise) ops.unrefOpPromise(this._acceptPromise);
        return this;
    }
}

// ── SocketAddress / BlockList (pure address arithmetic) ─────────────────

function ipToBigInt(address, family) {
    if (family === 'ipv4') {
        const parts = address.split('.').map(Number);
        return BigInt(((parts[0] << 24) >>> 0) + (parts[1] << 16) + (parts[2] << 8) + parts[3]);
    }
    // Normalize IPv6 through expansion of '::'.
    let addr = address;
    if (addr.includes('.')) {
        const lastColon = addr.lastIndexOf(':');
        const v4 = addr.slice(lastColon + 1).split('.').map(Number);
        addr = addr.slice(0, lastColon + 1)
            + ((v4[0] << 8) + v4[1]).toString(16) + ':' + ((v4[2] << 8) + v4[3]).toString(16);
    }
    const sides = addr.split('::');
    const head = sides[0] ? sides[0].split(':') : [];
    const tail = sides.length === 2 && sides[1] ? sides[1].split(':') : [];
    const groups = [...head, ...Array(8 - head.length - tail.length).fill('0'), ...tail];
    return groups.reduce((acc, g) => (acc << 16n) + BigInt(parseInt(g, 16) || 0), 0n);
}

export class SocketAddress {
    constructor(options = {}) {
        const family = (options.family || 'ipv4').toLowerCase();
        if (family !== 'ipv4' && family !== 'ipv6') {
            const err = new TypeError(
                `The argument 'options.family' must be one of: 'ipv4', 'ipv6'. Received ${receivedRepr(options.family)}`);
            err.code = 'ERR_INVALID_ARG_VALUE';
            throw err;
        }
        this.family = family;
        this.address = options.address || (family === 'ipv4' ? '127.0.0.1' : '::');
        this.port = options.port !== undefined ? validatePort(options.port) : 0;
        this.flowlabel = options.flowlabel || 0;
    }
}

export class BlockList {
    constructor() {
        this.rules = [];
        this._ranges = [];
    }

    static isBlockList(value) {
        return value instanceof BlockList;
    }

    _family(family) {
        if (family === undefined) return 'ipv4';
        if (typeof family !== 'string') {
            const err = new TypeError(
                `The "family" argument must be of type string. Received ${receivedRepr(family)}`);
            err.code = 'ERR_INVALID_ARG_TYPE';
            throw err;
        }
        const f = family.toLowerCase();
        if (f !== 'ipv4' && f !== 'ipv6') {
            const err = new TypeError(
                `The argument 'family' must be one of: 'ipv4', 'ipv6'. Received '${family}'`);
            err.code = 'ERR_INVALID_ARG_VALUE';
            throw err;
        }
        return f;
    }

    _validateAddress(address, name) {
        if (typeof address !== 'string') {
            const err = new TypeError(
                `The "${name}" argument must be of type string. Received ${receivedRepr(address)}`);
            err.code = 'ERR_INVALID_ARG_TYPE';
            throw err;
        }
    }

    addAddress(address, family) {
        if (address instanceof SocketAddress) {
            family = address.family;
            address = address.address;
        } else {
            this._validateAddress(address, 'address');
            family = this._family(family);
        }
        const value = ipToBigInt(address, family);
        this._ranges.push({ family, start: value, end: value });
        this.rules.push(`Address: ${family.toUpperCase()} ${address}`);
    }

    addRange(start, end, family) {
        if (start instanceof SocketAddress) {
            family = start.family;
            end = end instanceof SocketAddress ? end.address : end;
            start = start.address;
        } else {
            this._validateAddress(start, 'start');
            family = this._family(family);
            if (end instanceof SocketAddress) end = end.address;
        }
        this._validateAddress(end, 'end');
        this._ranges.push({
            family,
            start: ipToBigInt(start, family),
            end: ipToBigInt(end, family),
        });
        this.rules.push(`Range: ${family.toUpperCase()} ${start}-${end}`);
    }

    addSubnet(network, prefix, family) {
        if (network instanceof SocketAddress) {
            family = network.family;
            network = network.address;
        } else {
            this._validateAddress(network, 'network');
            family = this._family(family);
        }
        const bits = family === 'ipv4' ? 32 : 128;
        if (typeof prefix !== 'number') {
            const err = new TypeError(
                `The "prefix" argument must be of type number. Received ${receivedRepr(prefix)}`);
            err.code = 'ERR_INVALID_ARG_TYPE';
            throw err;
        }
        if (!Number.isInteger(prefix) || prefix < 0 || prefix > bits) {
            const err = new RangeError(
                `The value of "prefix" is out of range. It must be >= 0 and <= ${bits}. Received ${prefix}`);
            err.code = 'ERR_OUT_OF_RANGE';
            throw err;
        }
        const base = ipToBigInt(network, family);
        const mask = ((1n << BigInt(bits - prefix)) - 1n);
        const start = base & ~mask;
        this._ranges.push({ family, start, end: start + mask });
        this.rules.push(`Subnet: ${family.toUpperCase()} ${network}/${prefix}`);
    }

    check(address, family) {
        if (address instanceof SocketAddress) {
            family = address.family;
            address = address.address;
        } else {
            family = String(family || 'ipv4').toLowerCase();
        }
        if (typeof address !== 'string') return false;
        if (family === 'ipv4' && !isIPv4(address)) return false;
        if (family === 'ipv6' && !isIPv6(address)) return false;
        const value = ipToBigInt(address, family);
        const matches = (fam, v) => this._ranges.some((r) =>
            r.family === fam && v >= r.start && v <= r.end);
        if (matches(family, value)) return true;
        // IPv4-mapped IPv6 cross-matching: ::ffff:a.b.c.d and a.b.c.d name
        // the same host on both rule families.
        const V4_MAPPED = 0xffffn << 32n;
        if (family === 'ipv6' && (value >> 32n) === 0xffffn) {
            return matches('ipv4', value & 0xffffffffn);
        }
        if (family === 'ipv4') {
            return matches('ipv6', V4_MAPPED | value);
        }
        return false;
    }
}

if (Symbol.asyncDispose) {
    ServerImpl.prototype[Symbol.asyncDispose] = function asyncDispose() {
        return new Promise((resolve) => this.close(() => resolve()));
    };
    SocketImpl.prototype[Symbol.asyncDispose] = function asyncDispose() {
        this.destroy();
        return Promise.resolve();
    };
}

export const Socket = callable(SocketImpl);
export const Server = callable(ServerImpl);
// Legacy alias predating the Socket name.
export const Stream = Socket;

export function connect(...args) {
    const socket = new SocketImpl(typeof args[0] === 'object' ? args[0] : undefined);
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
    Stream,
    SocketAddress,
    BlockList,
    connect,
    createConnection,
    createServer,
};
