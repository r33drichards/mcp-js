// node:dgram — UDP over the loopback-only TCP/UDP capability (see
// net_tcp.rs). Without the capability, createSocket throws the standard
// capability-model error. With it, sockets bind and exchange datagrams on
// 127.0.0.1 only — the Rust side pins every bind and send target to the
// loopback interface. Multicast and broadcast configuration is accepted
// (validated, then a no-op) since loopback traffic never leaves the host.

import { EventEmitter } from 'node:events';
import { Buffer } from 'node:buffer';
import { isIPv4, isIPv6 } from 'node:net';

const ops = globalThis.__mcpV8NetOps;

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
        if (this._bound || this._closed) {
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
        const wantV6 = this.type === 'udp6';
        const host = options.address || (wantV6 ? '::1' : '127.0.0.1');
        let info;
        try {
            info = JSON.parse(ops.udpBind(String(host), (Number(options.port) || 0) >>> 0));
        } catch (error) {
            const err = new Error(String(error && error.message || error));
            const match = /\((E[A-Z]+)\)/.exec(err.message);
            if (match) err.code = match[1];
            Promise.resolve().then(() => this.emit('error', err));
            return this;
        }
        this._rid = info.rid;
        this._bound = true;
        this._address = {
            address: info.address,
            port: info.port,
            family: info.family,
        };
        Promise.resolve().then(() => this.emit('listening'));
        this._recvLoop(info.rid);
        return this;
    }

    async _recvLoop(rid) {
        while (this._rid === rid && !this._closed) {
            let result;
            try {
                result = JSON.parse(await ops.udpRecv(rid));
            } catch {
                break;
            }
            if (this._rid !== rid || this._closed) break;
            if (result.closed) break;
            if (result.error) {
                this.emit('error', new Error(result.error));
                break;
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

    connect(port, address, callback) {
        if (typeof address === 'function') {
            callback = address;
            address = undefined;
        }
        const validPort = validatePort(port);
        if (this._connectState !== CONNECT_STATE_DISCONNECTED) {
            throw nodeError(Error, 'ERR_SOCKET_DGRAM_IS_CONNECTED', 'Already connected');
        }
        this._connectState = CONNECT_STATE_CONNECTING;
        this._connectedTo = {
            port: validPort,
            address: address || (this.type === 'udp6' ? '::1' : '127.0.0.1'),
        };
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
        if (typeof rest[0] === 'number' && typeof rest[1] === 'number') {
            [offset, length] = rest;
            if (rest[2] !== undefined || rest.length > 2) port = rest[2];
            if (typeof rest[3] === 'string') address = rest[3];
            if (typeof rest[rest.length - 1] === 'function') cb = rest[rest.length - 1];
        } else {
            if (typeof rest[0] === 'number') port = rest[0];
            if (typeof rest[1] === 'string') address = rest[1];
            if (typeof rest[rest.length - 1] === 'function') cb = rest[rest.length - 1];
            if (rest.length > 0 && typeof rest[0] !== 'number'
                && typeof rest[0] !== 'function' && typeof rest[0] !== 'string'
                && rest[0] !== undefined) {
                port = rest[0]; // let validatePort reject it below
            }
        }

        const connected = this._connectState === CONNECT_STATE_CONNECTED
            || this._connectState === CONNECT_STATE_CONNECTING;
        if (connected && port !== undefined) {
            throw nodeError(Error, 'ERR_SOCKET_DGRAM_IS_CONNECTED', 'Already connected');
        }

        let buf;
        if (Array.isArray(msg)) {
            buf = Buffer.concat(msg.map((part) => toSendBuffer(part)));
        } else {
            buf = toSendBuffer(msg);
            if (offset !== undefined && length !== undefined) {
                buf = buf.subarray(offset, offset + length);
            }
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

        if (!this._bound) this.bind(0);
        const rid = this._rid;
        const sent = buf.length;
        const finish = (error) => {
            if (typeof cb === 'function') cb(error || null, error ? undefined : sent);
            else if (error) this.emit('error', error);
        };
        if (rid === null) {
            // bind() failed synchronously and emitted its own error.
            Promise.resolve().then(() => finish(new Error('dgram: socket not bound')));
            return;
        }
        ops.udpSend(rid, String(address || '127.0.0.1'), port >>> 0,
            buf.toString('base64')).then(
            () => finish(null),
            (error) => finish(new Error(String(error && error.message || error))));
    }

    address() {
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
        const rid = this._rid;
        this._rid = null;
        if (rid !== null && ops) ops.udpClose(rid);
        if (typeof cb === 'function') this.once('close', cb);
        Promise.resolve().then(() => this.emit('close'));
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
        if (typeof interfaceAddress !== 'string') {
            throw nodeError(TypeError, 'ERR_INVALID_ARG_TYPE',
                `The "interfaceAddress" argument must be of type string. Received ${receivedRepr(interfaceAddress)}`);
        }
        if (!this._healthy()) throw new Error('setMulticastInterface EBADF');
        if (!isIPv4(interfaceAddress) && !isIPv6(interfaceAddress)) {
            throw new Error('setMulticastInterface EINVAL');
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

    getRecvBufferSize() {
        if (!this._healthy()) throw new Error('getRecvBufferSize EBADF');
        return this._recvBufferSize;
    }

    getSendBufferSize() {
        if (!this._healthy()) throw new Error('getSendBufferSize EBADF');
        return this._sendBufferSize;
    }

    ref() { return this; }
    unref() { return this; }
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
