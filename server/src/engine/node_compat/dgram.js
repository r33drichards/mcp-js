// node:dgram — UDP over the loopback-only TCP/UDP capability (see
// net_tcp.rs). Without the capability, createSocket throws the standard
// capability-model error. With it, sockets bind and exchange datagrams on
// 127.0.0.1 only — the Rust side pins every bind and send target to the
// loopback interface. Multicast and broadcast configuration is accepted
// (validated, then a no-op) since loopback traffic never leaves the host.

import { EventEmitter } from 'node:events';
import { Buffer } from 'node:buffer';
import { isIPv4, isIPv6 } from 'node:net';
import dns from 'node:dns';

const ops = globalThis.__mcpV8NetOps;

// Shares the open-refed-handle count with node:net (see net.js) so the
// corpus harness's end-of-test check waits for dgram sockets too.
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

function syncHandle(self, open) {
    self._handleOpen = open;
    const contribution = (self._handleOpen && !self._unrefed) ? 1 : 0;
    handleRegistry.refed += contribution - (self._handleContrib || 0);
    self._handleContrib = contribution;
}


function callable(Cls) {
    return new Proxy(Cls, {
        apply: (target, _thisArg, args) => new target(...args),
    });
}

function nodeError(Ctor, code, message) {
    const err = new Ctor(message);
    err.code = code;
    return err;
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
    if (type === 'function') return `function ${value.name || ''}`.trim();
    if (type === 'bigint') return `type bigint (${value}n)`;
    return `type ${type} (${String(value)})`;
}

function validateNumberArg(value, name) {
    if (typeof value !== 'number') {
        throw nodeError(TypeError, 'ERR_INVALID_ARG_TYPE',
            `The "${name}" argument must be of type number. Received ${receivedRepr(value)}`);
    }
}

function validatePort(port) {
    const value = typeof port === 'number' ? port : Number.NaN;
    if (!Number.isInteger(value) || value <= 0 || value >= 65536) {
        throw nodeError(RangeError, 'ERR_SOCKET_BAD_PORT',
            `Port should be > 0 and < 65536. Received ${receivedRepr(port)}.`);
    }
    return value;
}

// The sync corpus APIs take only literal IPs — no resolution happens.
function validateSyncAddress(address) {
    if (address === undefined) return;
    if (typeof address !== 'string') {
        throw nodeError(TypeError, 'ERR_INVALID_ARG_TYPE',
            `The "address" argument must be of type string. Received ${receivedRepr(address)}`);
    }
    if (!isIPv4(address) && !isIPv6(address)) {
        throw nodeError(TypeError, 'ERR_INVALID_ARG_VALUE',
            `The argument 'address' is invalid. Received '${address}'`);
    }
}

function validateBindPort(port) {
    if (port === undefined) return 0;
    const value = typeof port === 'number' || typeof port === 'string'
        ? Number(port) : Number.NaN;
    if (!Number.isInteger(value) || value < 0 || value >= 65536) {
        throw nodeError(RangeError, 'ERR_SOCKET_BAD_PORT',
            `Port should be >= 0 and < 65536. Received ${receivedRepr(port)}.`);
    }
    return value;
}

function toSendBuffer(msg) {
    if (typeof msg === 'string') return Buffer.from(msg);
    if (Buffer.isBuffer(msg)) return msg;
    if (ArrayBuffer.isView(msg)) {
        return Buffer.from(msg.buffer, msg.byteOffset, msg.byteLength);
    }
    throw nodeError(TypeError, 'ERR_INVALID_ARG_TYPE',
        'The "buffer" argument must be of type string or an instance of ' +
        `Buffer, TypedArray, or DataView. Received ${receivedRepr(msg)}`);
}

const CONNECT_STATE_DISCONNECTED = 0;
const CONNECT_STATE_CONNECTING = 1;
const CONNECT_STATE_CONNECTED = 2;

const LOOPBACK_OK = new Set(['localhost', '127.0.0.1', '::1', '0.0.0.0', '::', '::0', '*', '']);

// Node-shaped error for an address the loopback-only transport can never
// reach: a foreign IP is EADDRNOTAVAIL (bind) and an unresolvable
// hostname ENOTFOUND, matching what Node reports for the same inputs.
function addressUnavailable(syscall, host) {
    const value = String(host);
    if (LOOPBACK_OK.has(value)) return null;
    if (isIPv4(value) || isIPv6(value)) {
        if (value.startsWith('127.') || value === '::1') return null;
        const err = new Error(`${syscall} EADDRNOTAVAIL ${value}`);
        err.code = 'EADDRNOTAVAIL';
        err.errno = -99;
        err.syscall = syscall;
        err.address = value;
        return err;
    }
    const err = new Error(`getaddrinfo ENOTFOUND ${value}`);
    err.code = 'ENOTFOUND';
    err.errno = -3008;
    err.syscall = 'getaddrinfo';
    err.hostname = value;
    return err;
}

class SocketImpl extends EventEmitter {
    constructor(typeOrOptions, listener) {
        super();
        let options = null;
        if (typeof typeOrOptions === 'string') {
            options = { type: typeOrOptions };
        } else if (typeOrOptions !== null && typeof typeOrOptions === 'object'
            && !Array.isArray(typeOrOptions) && !(typeOrOptions instanceof String)) {
            options = typeOrOptions;
        }
        const type = options && options.type;
        if (type !== 'udp4' && type !== 'udp6') {
            throw nodeError(TypeError, 'ERR_SOCKET_BAD_TYPE',
                'Bad socket type specified. Valid types are: udp4, udp6');
        }
        this.type = type;
        this._rid = null;
        this._address = null;
        this._bound = false;
        this._closed = false;
        this._connectState = CONNECT_STATE_DISCONNECTED;
        this._connectedTo = null;
        if (options && options.recvBufferSize !== undefined) {
            validateNumberArg(options.recvBufferSize, 'options.recvBufferSize');
        }
        if (options && options.sendBufferSize !== undefined) {
            validateNumberArg(options.sendBufferSize, 'options.sendBufferSize');
        }
        this._recvBufferSize = (options && options.recvBufferSize) || 65536;
        this._sendBufferSize = (options && options.sendBufferSize) || 65536;
        this._sendBlockList = (options && options.sendBlockList) || null;
        this._receiveBlockList = (options && options.receiveBlockList) || null;
        if (options && 'lookup' in options && typeof options.lookup !== 'function') {
            const err = new TypeError(
                `The "lookup" argument must be of type function. Received ${receivedRepr(options.lookup)}`);
            err.code = 'ERR_INVALID_ARG_TYPE';
            throw err;
        }
        this._lookup = (options && options.lookup) || null;
        this._pendingSendBytes = 0;
        this._pendingSendCount = 0;
        if (typeof listener === 'function') this.on('message', listener);
        const signal = options && options.signal;
        if (signal !== undefined) {
            if (!signal || typeof signal.addEventListener !== 'function'
                || typeof signal.aborted !== 'boolean') {
                throw nodeError(TypeError, 'ERR_INVALID_ARG_TYPE',
                    `The "options.signal" property must be an instance of AbortSignal. Received ${receivedRepr(signal)}`);
            }
            if (signal.aborted) {
                Promise.resolve().then(() => { if (!this._closed) this.close(); });
            } else {
                signal.addEventListener('abort', () => {
                    if (!this._closed) this.close();
                }, { once: true });
            }
        }
    }

    // Resolve a bind/send target through the socket's lookup function, or
    // the live dns.lookup so tests that monkeypatch it observe the call.
    // Literal IPs and localhost shortcut only when no custom lookup is set.
    _resolveAddress(host, callback) {
        const family = this.type === 'udp6' ? 6 : 4;
        if (this._lookup) {
            this._lookup(host, family, callback);
            return;
        }
        if (isIPv4(host) || isIPv6(host)) {
            Promise.resolve().then(() => callback(null, host, family));
            return;
        }
        if (host === 'localhost') {
            Promise.resolve().then(() =>
                callback(null, family === 6 ? '::1' : '127.0.0.1', family));
            return;
        }
        dns.lookup(host, family, callback);
    }

    _blockedBySendList(address) {
        if (!this._sendBlockList || typeof this._sendBlockList.check !== 'function') {
            return false;
        }
        const ip = address === 'localhost' ? '127.0.0.1' : address;
        return this._sendBlockList.check(ip, isIPv6(ip) ? 'ipv6' : 'ipv4');
    }

    _healthy() {
        return this._bound && !this._closed && this._rid !== null;
    }

    // Node's health check: only a closed socket is "not running" — an
    // unbound one is implicitly bound by the operations that need it.
    _requireRunning() {
        if (this._closed) {
            throw nodeError(Error, 'ERR_SOCKET_DGRAM_NOT_RUNNING', 'Not running');
        }
    }

    bind(...args) {
        if (this._bound || this._binding || this._closed) {
            throw nodeError(Error, 'ERR_SOCKET_ALREADY_BOUND',
                'Socket is already bound');
        }
        let options = {};
        let cb;
        if (typeof args[0] === 'object' && args[0] !== null) {
            options = args[0];
            cb = args[1];
        } else {
            if (typeof args[0] === 'number' || typeof args[0] === 'string') {
                options.port = Number(args[0]) || 0;
            } else if (typeof args[0] === 'function') {
                cb = args[0];
            }
            if (typeof args[1] === 'string') options.address = args[1];
            if (typeof args[args.length - 1] === 'function') cb = args[args.length - 1];
        }
        if (typeof cb === 'function') this.once('listening', cb);
        const failBind = (err) => {
            // Node removes the pending listening listener on a failed bind
            // so a retry does not stack callbacks.
            if (typeof cb === 'function') this.removeListener('listening', cb);
            Promise.resolve().then(() => this.emit('error', err));
            return this;
        };
        const wantV6 = this.type === 'udp6';
        const requested = options.address;
        // Node resolves the bind address through the lookup function even
        // for a default bind, so custom lookups observe the call.
        this._binding = true;
        this._resolveAddress(requested || (wantV6 ? '::' : '0.0.0.0'), (error, resolved) => {
            this._binding = false;
            // A close during bind still lets already-queued sends flush —
            // Node guarantees a send issued before close goes out.
            const queued = this._boundQueue && this._boundQueue.length > 0;
            if (this._closed && !queued) return;
            if (error) return failBind(error);
            // Node-shaped failure for an address the loopback sandbox can
            // never bind: a foreign IP is EADDRNOTAVAIL.
            const shaped = addressUnavailable('bind', resolved);
            if (shaped) return failBind(shaped);
            let info;
            try {
                info = JSON.parse(
                    ops.udpBind(String(resolved), (Number(options.port) || 0) >>> 0));
            } catch (opError) {
                const err = new Error(String(opError && opError.message || opError));
                const match = /\((E[A-Z]+)\)/.exec(err.message);
                if (match) err.code = match[1];
                return failBind(err);
            }
            this._rid = info.rid;
            this._bound = true;
            // The transport pins to loopback; report the wildcard the caller
            // asked for (Node's observable behavior for a default bind).
            const wildcard = requested === undefined || requested === ''
                || requested === '0.0.0.0' || requested === '::';
            this._address = {
                address: wildcard ? (wantV6 ? '::' : '0.0.0.0') : info.address,
                port: info.port,
                family: info.family,
            };
            syncHandle(this, true);
            if (!this._closed) this.emit('listening');
            this._drainBoundQueue();
            if (!this._closed) this._recvLoop(info.rid);
        });
        return this;
    }

    async _recvLoop(rid) {
        while (this._rid === rid && !this._closed) {
            let result;
            try {
                // close() awaits this promise so the OS socket is provably
                // released before 'close' fires — a test rebinding the same
                // port from its close handler must not hit EADDRINUSE.
                this._recvPromise = ops.udpRecv(rid);
                if (this._unrefed && ops.unrefOpPromise) {
                    ops.unrefOpPromise(this._recvPromise);
                }
                result = JSON.parse(await this._recvPromise);
            } catch {
                break;
            }
            if (this._rid !== rid || this._closed) break;
            if (result.closed) break;
            if (result.error) {
                this.emit('error', new Error(result.error));
                break;
            }
            if (this._receiveBlockList
                && typeof this._receiveBlockList.check === 'function'
                && this._receiveBlockList.check(
                    result.address, result.family === 'IPv6' ? 'ipv6' : 'ipv4')) {
                continue;
            }
            const msg = Buffer.from(result.data, 'base64');
            this.emit('message', msg, {
                address: result.address,
                family: result.family,
                port: result.port,
                size: msg.length,
            });
        }
    }

    // Corpus-fork extension: synchronous bind returning the resolved
    // address; the 'listening' event is deferred a tick and suppressed if
    // the socket closes first.
    bindSync(options) {
        if (this._bound || this._binding || this._closed) {
            throw nodeError(Error, 'ERR_SOCKET_ALREADY_BOUND',
                'Socket is already bound');
        }
        const wantV6 = this.type === 'udp6';
        const requested = options && options.address;
        validateSyncAddress(requested);
        const port = validateBindPort(options && options.port);
        const host = requested || (wantV6 ? '::1' : '127.0.0.1');
        const shaped = addressUnavailable('bind', host);
        if (shaped) throw shaped;
        let info;
        try {
            info = JSON.parse(ops.udpBind(String(host), port >>> 0));
        } catch (error) {
            const err = new Error(String(error && error.message || error));
            const match = /\((E[A-Z]+)\)/.exec(err.message);
            if (match) {
                err.code = match[1];
                err.message = `bind ${match[1]} ${host}${port ? ':' + port : ''}`;
            }
            err.syscall = 'bind';
            err.address = host;
            throw err;
        }
        this._rid = info.rid;
        this._bound = true;
        const wildcard = requested === undefined || requested === ''
            || requested === '0.0.0.0' || requested === '::';
        this._address = {
            address: wildcard ? (wantV6 ? '::' : '0.0.0.0') : info.address,
            port: info.port,
            family: info.family,
        };
        syncHandle(this, true);
        Promise.resolve().then(() => {
            if (this._closed) return;
            this.emit('listening');
            this._drainBoundQueue();
        });
        this._recvLoop(info.rid);
        return { ...this._address };
    }

    // Corpus-fork extension: synchronous connect (binding first if needed);
    // the 'connect' event is deferred a tick and suppressed on close.
    connectSync(port, address) {
        const validPort = validatePort(port);
        validateSyncAddress(address);
        if (this._connectState !== CONNECT_STATE_DISCONNECTED) {
            throw nodeError(Error, 'ERR_SOCKET_DGRAM_IS_CONNECTED', 'Already connected');
        }
        const target = address || (this.type === 'udp6' ? '::1' : '127.0.0.1');
        if (this._blockedBySendList(target)) {
            throw nodeError(Error, 'ERR_IP_BLOCKED', `IP ${target} is blocked`);
        }
        if (!this._bound) this.bindSync();
        this._connectState = CONNECT_STATE_CONNECTED;
        this._connectedTo = {
            port: validPort,
            address: address || (this.type === 'udp6' ? '::1' : '127.0.0.1'),
        };
        Promise.resolve().then(() => {
            if (!this._closed) this.emit('connect');
        });
        return this;
    }

    connect(port, address, callback) {
        if (typeof address === 'function') {
            callback = address;
            address = undefined;
        }
        const validPort = validatePort(port);
        if (this._connectState !== CONNECT_STATE_DISCONNECTED) {
            throw nodeError(Error, 'ERR_SOCKET_DGRAM_IS_CONNECTED', 'Already connected');
        }
        const target = address || (this.type === 'udp6' ? '::1' : '127.0.0.1');
        if (this._blockedBySendList(target)) {
            const err = nodeError(Error, 'ERR_IP_BLOCKED', `IP ${target} is blocked`);
            Promise.resolve().then(() => {
                if (typeof callback === 'function') callback(err);
                else this.emit('error', err);
            });
            return this;
        }
        this._connectState = CONNECT_STATE_CONNECTING;
        this._connectedTo = { port: validPort, address: target };
        if (!this._bound) this.bind(0);
        Promise.resolve().then(() => {
            if (this._closed) return;
            this._connectState = CONNECT_STATE_CONNECTED;
            if (typeof callback === 'function') callback();
            this.emit('connect');
        });
        return this;
    }

    disconnect() {
        if (this._connectState !== CONNECT_STATE_CONNECTED) {
            throw nodeError(Error, 'ERR_SOCKET_DGRAM_NOT_CONNECTED', 'Not connected');
        }
        this._connectState = CONNECT_STATE_DISCONNECTED;
        this._connectedTo = null;
    }

    remoteAddress() {
        if (this._connectState !== CONNECT_STATE_CONNECTED || !this._connectedTo) {
            throw nodeError(Error, 'ERR_SOCKET_DGRAM_NOT_CONNECTED', 'Not connected');
        }
        return {
            address: this._connectedTo.address,
            port: this._connectedTo.port,
            family: this.type === 'udp6' ? 'IPv6' : 'IPv4',
        };
    }

    send(msg, ...rest) {
        // Signatures: (msg, port[, address][, cb]),
        // (msg, offset, length, port[, address][, cb]), (msg[, cb]) when
        // connected.
        let offset;
        let length;
        let port;
        let address;
        let cb;
        // Two leading numbers are offset/length only in the explicit
        // 5-argument form (third number = port) or on a connected socket;
        // otherwise the pattern is (port, address, cb) with a bad address.
        if (typeof rest[0] === 'number' && typeof rest[1] === 'number'
            && (typeof rest[2] === 'number'
                || this._connectState !== CONNECT_STATE_DISCONNECTED)) {
            [offset, length] = rest;
            // (msg, offset, length, cb) is the connected form — a trailing
            // function is the callback, not a port.
            if (rest.length > 2 && typeof rest[2] !== 'function') port = rest[2];
            if (typeof rest[3] === 'string') address = rest[3];
            if (typeof rest[rest.length - 1] === 'function') cb = rest[rest.length - 1];
        } else {
            if (typeof rest[0] === 'number') port = rest[0];
            if (rest.length > 1 && rest[1] !== undefined
                && typeof rest[1] !== 'function') {
                address = rest[1]; // non-strings rejected below
            }
            if (typeof rest[rest.length - 1] === 'function') cb = rest[rest.length - 1];
            if (rest.length > 0 && typeof rest[0] !== 'number'
                && typeof rest[0] !== 'function' && typeof rest[0] !== 'string'
                && rest[0] !== undefined) {
                port = rest[0]; // let validatePort reject it below
            }
        }

        // Node validates the buffer (type and bounds) before objecting to a
        // port on a connected socket.
        let buf;
        if (Array.isArray(msg)) {
            try {
                buf = Buffer.concat(msg.map((part) => toSendBuffer(part)));
            } catch {
                throw nodeError(TypeError, 'ERR_INVALID_ARG_TYPE',
                    'The "buffer list arguments" argument must be of type string ' +
                    'or an instance of Buffer, TypedArray, or DataView. ' +
                    'Received an instance of Array');
            }
        } else {
            buf = toSendBuffer(msg);
            if (offset !== undefined && length !== undefined) {
                if (offset > buf.length) {
                    throw nodeError(RangeError, 'ERR_BUFFER_OUT_OF_BOUNDS',
                        '"offset" is outside of buffer bounds');
                }
                if (length > buf.length - offset) {
                    throw nodeError(RangeError, 'ERR_BUFFER_OUT_OF_BOUNDS',
                        '"length" is outside of buffer bounds');
                }
                buf = buf.subarray(offset, offset + length);
            }
        }

        const connected = this._connectState === CONNECT_STATE_CONNECTED
            || this._connectState === CONNECT_STATE_CONNECTING;
        if (connected && port !== undefined) {
            throw nodeError(Error, 'ERR_SOCKET_DGRAM_IS_CONNECTED', 'Already connected');
        }

        if (connected) {
            port = this._connectedTo.port;
            address = address || this._connectedTo.address;
        } else {
            port = validatePort(port);
        }
        if (address !== undefined && address !== null && typeof address !== 'string') {
            throw nodeError(TypeError, 'ERR_INVALID_ARG_TYPE',
                `The "address" argument must be of type string. Received ${receivedRepr(address)}`);
        }

        const sent = buf.length;
        this._pendingSendBytes += sent;
        this._pendingSendCount += 1;
        const sendTarget = String(address || '127.0.0.1');
        const issuedBeforeClose = !this._closed;
        const finish = (error) => {
            this._pendingSendBytes -= sent;
            this._pendingSendCount -= 1;
            // A callback for a send issued before close() that lands after
            // it is dropped, like Node's canceled uv send requests; errors
            // for such sends are not propagated either.
            if (issuedBeforeClose && this._closed) {
                if (this._pendingSendCount === 0 && this._closeWhenDrained) {
                    const complete = this._closeWhenDrained;
                    this._closeWhenDrained = null;
                    complete();
                }
                return;
            }
            if (error) {
                // Known transport codes get Node's exact error shape.
                if (error.code && /^E[A-Z]+$/.test(error.code)
                    && error.code !== 'ENOTFOUND') {
                    error.message = `send ${error.code} ${sendTarget}:${port}`;
                    error.stack = `Error: ${error.message}`;
                    error.syscall = 'send';
                }
                error.address = error.address || sendTarget;
                if (error.port === undefined && error.code !== 'ENOTFOUND') {
                    error.port = port;
                }
            }
            if (typeof cb === 'function') cb(error || null, error ? undefined : sent);
            else if (error) this.emit('error', error);
            if (this._pendingSendCount === 0 && this._closeWhenDrained) {
                const complete = this._closeWhenDrained;
                this._closeWhenDrained = null;
                complete();
            }
        };
        if (this._blockedBySendList(sendTarget)) {
            Promise.resolve().then(() => finish(
                nodeError(Error, 'ERR_IP_BLOCKED', `IP ${sendTarget} is blocked`)));
            return;
        }
        this._whenBound(() => {
            const rid = this._rid;
            if (rid === null) {
                const err = this._closed
                    ? nodeError(Error, 'ERR_SOCKET_DGRAM_NOT_RUNNING', 'Not running')
                    : new Error('dgram: socket not bound');
                Promise.resolve().then(() => finish(err));
                return;
            }
            this._resolveAddress(sendTarget, (lookupError, resolved) => {
                if (lookupError) { finish(lookupError); return; }
                ops.udpSend(rid, String(resolved), port >>> 0,
                    buf.toString('base64')).then(
                    () => finish(null),
                    (error) => {
                        const err = new Error(String(error && error.message || error));
                        const match = /\((E[A-Z]+)\)/.exec(err.message);
                        if (match) err.code = match[1];
                        finish(err);
                    });
            });
        });
    }

    // Legacy strict-signature variant of send().
    sendto(buffer, offset, length, port, address, callback) {
        if (typeof offset !== 'number') {
            throw nodeError(TypeError, 'ERR_INVALID_ARG_TYPE',
                `The "offset" argument must be of type number. Received ${receivedRepr(offset)}`);
        }
        if (typeof length !== 'number') {
            throw nodeError(TypeError, 'ERR_INVALID_ARG_TYPE',
                `The "length" argument must be of type number. Received ${receivedRepr(length)}`);
        }
        if (typeof port !== 'number') {
            throw nodeError(TypeError, 'ERR_INVALID_ARG_TYPE',
                `The "port" argument must be of type number. Received ${receivedRepr(port)}`);
        }
        if (typeof address !== 'string') {
            throw nodeError(TypeError, 'ERR_INVALID_ARG_TYPE',
                `The "address" argument must be of type string. Received ${receivedRepr(address)}`);
        }
        return this.send(buffer, offset, length, port, address, callback);
    }

    _whenBound(fn) {
        if (this._bound) { fn(); return; }
        (this._boundQueue ||= []).push(fn);
        if (!this._binding && !this._closed) this.bind(0);
    }

    _drainBoundQueue() {
        const queue = this._boundQueue || [];
        this._boundQueue = [];
        for (const fn of queue) fn();
    }

    address() {
        this._requireRunning();
        if (!this._healthy() || !this._address) {
            throw nodeError(Error, 'EBADF', 'getsockname EBADF');
        }
        return { ...this._address };
    }

    close(cb) {
        if (this._closed) {
            const err = nodeError(Error, 'ERR_SOCKET_DGRAM_NOT_RUNNING', 'Not running');
            if (typeof cb === 'function') { Promise.resolve().then(() => cb(err)); return this; }
            throw err;
        }
        this._closed = true;
        if (typeof cb === 'function') this.once('close', cb);
        // Sends queued behind an in-flight bind flush when the bind resolves;
        // without one they resolve now ("Not running").
        if (!this._binding) this._drainBoundQueue();
        // Node drains queued sends before tearing the socket down.
        const complete = async () => {
            syncHandle(this, false);
            const rid = this._rid;
            this._rid = null;
            if (rid !== null && ops) ops.udpClose(rid);
            if (this._recvPromise) {
                try { await this._recvPromise; } catch { /* released */ }
            }
            this.emit('close');
        };
        if (this._pendingSendCount > 0) {
            this._closeWhenDrained = complete;
        } else {
            Promise.resolve().then(complete);
        }
        return this;
    }

    setBroadcast(_flag) {
        if (!this._healthy()) throw new Error('setBroadcast EBADF');
    }

    setTTL(ttl) {
        validateNumberArg(ttl, 'ttl');
        if (!this._healthy()) throw new Error('setTTL EBADF');
        if (ttl < 1 || ttl > 255 || !Number.isInteger(ttl)) {
            throw new Error('setTTL EINVAL');
        }
        return ttl;
    }

    setMulticastTTL(ttl) {
        validateNumberArg(ttl, 'ttl');
        if (!this._healthy()) throw new Error('setMulticastTTL EBADF');
        if (ttl < 0 || ttl > 255 || !Number.isInteger(ttl)) {
            throw new Error('setMulticastTTL EINVAL');
        }
        return ttl;
    }

    setMulticastLoopback(flag) {
        if (!this._healthy()) throw new Error('setMulticastLoopback EBADF');
        return flag;
    }

    setMulticastInterface(interfaceAddress) {
        this._requireRunning();
        if (typeof interfaceAddress !== 'string') {
            throw nodeError(TypeError, 'ERR_INVALID_ARG_TYPE',
                `The "interfaceAddress" argument must be of type string. Received ${receivedRepr(interfaceAddress)}`);
        }
        if (!this._healthy()) throw new Error('setMulticastInterface EBADF');
        const invalid = this.type === 'udp6'
            ? !isIPv6(interfaceAddress)
            // The interface address must be unicast — the multicast range
            // (224/4) is rejected by the OS with EINVAL.
            : (!isIPv4(interfaceAddress)
                || Number(interfaceAddress.split('.')[0]) >= 224);
        if (invalid) {
            const err = new Error('setMulticastInterface EINVAL');
            err.code = 'EINVAL';
            err.syscall = 'setMulticastInterface';
            throw err;
        }
    }

    addMembership(multicastAddress, _interfaceAddress) {
        if (!multicastAddress) {
            throw nodeError(TypeError, 'ERR_MISSING_ARGS',
                'The "multicastAddress" argument must be specified');
        }
        this._requireRunning();
        if (!isIPv4(multicastAddress) && !isIPv6(multicastAddress)) {
            throw new Error('addMembership EINVAL');
        }
    }

    dropMembership(multicastAddress, _interfaceAddress) {
        if (!multicastAddress) {
            throw nodeError(TypeError, 'ERR_MISSING_ARGS',
                'The "multicastAddress" argument must be specified');
        }
        this._requireRunning();
        if (!isIPv4(multicastAddress) && !isIPv6(multicastAddress)) {
            throw new Error('dropMembership EINVAL');
        }
    }

    addSourceSpecificMembership(sourceAddress, groupAddress, _interfaceAddress) {
        if (typeof sourceAddress !== 'string') {
            throw nodeError(TypeError, 'ERR_INVALID_ARG_TYPE',
                `The "sourceAddress" argument must be of type string. Received ${receivedRepr(sourceAddress)}`);
        }
        if (typeof groupAddress !== 'string') {
            throw nodeError(TypeError, 'ERR_INVALID_ARG_TYPE',
                `The "groupAddress" argument must be of type string. Received ${receivedRepr(groupAddress)}`);
        }
        this._requireRunning();
        if (!isIPv4(sourceAddress) && !isIPv6(sourceAddress)) {
            throw nodeError(Error, 'EINVAL', 'addSourceSpecificMembership EINVAL');
        }
        if (!isIPv4(groupAddress) && !isIPv6(groupAddress)) {
            throw nodeError(Error, 'EINVAL', 'addSourceSpecificMembership EINVAL');
        }
    }

    dropSourceSpecificMembership(sourceAddress, groupAddress, _interfaceAddress) {
        if (typeof sourceAddress !== 'string') {
            throw nodeError(TypeError, 'ERR_INVALID_ARG_TYPE',
                `The "sourceAddress" argument must be of type string. Received ${receivedRepr(sourceAddress)}`);
        }
        if (typeof groupAddress !== 'string') {
            throw nodeError(TypeError, 'ERR_INVALID_ARG_TYPE',
                `The "groupAddress" argument must be of type string. Received ${receivedRepr(groupAddress)}`);
        }
        this._requireRunning();
        if (!isIPv4(sourceAddress) && !isIPv6(sourceAddress)) {
            throw nodeError(Error, 'EINVAL', 'dropSourceSpecificMembership EINVAL');
        }
        if (!isIPv4(groupAddress) && !isIPv6(groupAddress)) {
            throw nodeError(Error, 'EINVAL', 'dropSourceSpecificMembership EINVAL');
        }
    }

    setRecvBufferSize(size) {
        validateNumberArg(size, 'size');
        if (!this._healthy()) throw new Error('setRecvBufferSize EBADF');
        this._recvBufferSize = size;
    }

    setSendBufferSize(size) {
        validateNumberArg(size, 'size');
        if (!this._healthy()) throw new Error('setSendBufferSize EBADF');
        this._sendBufferSize = size;
    }

    getSendQueueSize() { return this._pendingSendBytes; }
    getSendQueueCount() { return this._pendingSendCount; }

    getRecvBufferSize() {
        if (!this._healthy()) throw new Error('getRecvBufferSize EBADF');
        return this._recvBufferSize;
    }

    getSendBufferSize() {
        if (!this._healthy()) throw new Error('getSendBufferSize EBADF');
        return this._sendBufferSize;
    }

    // Real ref/unref semantics: an unrefed socket's pending recv op no
    // longer holds the event loop open, so the run can drain like a Node
    // process exiting with unrefed handles.
    ref() {
        this._unrefed = false;
        syncHandle(this, this._handleOpen);
        if (this._recvPromise && ops.refOpPromise) ops.refOpPromise(this._recvPromise);
        return this;
    }

    unref() {
        this._unrefed = true;
        syncHandle(this, this._handleOpen);
        if (this._recvPromise && ops.unrefOpPromise) ops.unrefOpPromise(this._recvPromise);
        return this;
    }
}

if (Symbol.asyncDispose) {
    SocketImpl.prototype[Symbol.asyncDispose] = function asyncDispose() {
        if (this._closed) return Promise.resolve();
        return new Promise((resolve) => this.close(() => resolve()));
    };
}

export const Socket = callable(SocketImpl);

export function createSocket(typeOrOptions, listener) {
    if (!ops) {
        throw new Error(
            'dgram is not supported in this runtime: sandbox networking goes ' +
            'through the policy-gated fetch / WebSocket / node:http2 capabilities');
    }
    return new SocketImpl(typeOrOptions, listener);
}

export default {
    Socket,
    createSocket,
};
