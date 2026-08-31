// node:dgram — UDP over the loopback-only TCP/UDP capability (see
// net_tcp.rs). Without the capability, createSocket throws the standard
// capability-model error. With it, sockets bind and exchange datagrams on
// 127.0.0.1 only — the Rust side pins every bind and send target to the
// loopback interface.

import { EventEmitter } from 'node:events';
import { Buffer } from 'node:buffer';

const ops = globalThis.__mcpV8NetOps;

function callable(Cls) {
    return new Proxy(Cls, {
        apply: (target, _thisArg, args) => new target(...args),
    });
}

class SocketImpl extends EventEmitter {
    constructor(typeOrOptions, listener) {
        super();
        const options = typeof typeOrOptions === 'string'
            ? { type: typeOrOptions }
            : (typeOrOptions || {});
        this.type = options.type || 'udp4';
        this._rid = null;
        this._address = null;
        this._bound = false;
        this._closed = false;
        this._connectedTo = null;
        if (typeof listener === 'function') this.on('message', listener);
    }

    bind(...args) {
        if (this._bound) {
            const err = new Error('bind() called twice');
            err.code = 'ERR_SOCKET_ALREADY_BOUND';
            throw err;
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
        this._connectedTo = { port: Number(port), address: address || '127.0.0.1' };
        if (!this._bound) this.bind(0);
        if (typeof callback === 'function') {
            Promise.resolve().then(callback);
        }
        Promise.resolve().then(() => this.emit('connect'));
        return this;
    }

    disconnect() {
        this._connectedTo = null;
    }

    remoteAddress() {
        if (!this._connectedTo) {
            const err = new Error('Socket is not connected');
            err.code = 'ERR_SOCKET_DGRAM_NOT_CONNECTED';
            throw err;
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
        if (rest.length >= 3 && typeof rest[0] === 'number' && typeof rest[1] === 'number'
            && typeof rest[2] === 'number') {
            [offset, length, port] = rest;
            if (typeof rest[3] === 'string') address = rest[3];
            if (typeof rest[rest.length - 1] === 'function') cb = rest[rest.length - 1];
        } else {
            if (typeof rest[0] === 'number') port = rest[0];
            if (typeof rest[1] === 'string') address = rest[1];
            if (typeof rest[rest.length - 1] === 'function') cb = rest[rest.length - 1];
        }
        if (port === undefined && this._connectedTo) {
            port = this._connectedTo.port;
            address = address || this._connectedTo.address;
        }
        let buf = Buffer.isBuffer(msg)
            ? msg
            : Array.isArray(msg)
                ? Buffer.concat(msg.map((m) => Buffer.isBuffer(m) ? m : Buffer.from(m)))
                : Buffer.from(String(msg));
        if (offset !== undefined && length !== undefined) {
            buf = buf.subarray(offset, offset + length);
        }
        if (!this._bound) this.bind(0);
        const rid = this._rid;
        const finish = (error) => {
            if (typeof cb === 'function') cb(error || null);
            else if (error) this.emit('error', error);
        };
        if (rid === null) {
            // bind() failed synchronously and emitted its own error.
            Promise.resolve().then(() => finish(new Error('dgram: socket not bound')));
            return;
        }
        ops.udpSend(rid, String(address || '127.0.0.1'), (Number(port) || 0) >>> 0,
            buf.toString('base64')).then(
            () => finish(null),
            (error) => finish(new Error(String(error && error.message || error))));
    }

    address() {
        if (!this._bound || !this._address) {
            const err = new Error('getsockname EBADF');
            err.code = 'EBADF';
            throw err;
        }
        return { ...this._address };
    }

    close(cb) {
        if (this._closed) {
            const err = new Error('Not running');
            err.code = 'ERR_SOCKET_DGRAM_NOT_RUNNING';
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

    setBroadcast() {}
    setTTL(ttl) { return ttl; }
    setMulticastTTL(ttl) { return ttl; }
    setMulticastLoopback(flag) { return flag; }
    setMulticastInterface() {}
    addMembership() {}
    dropMembership() {}
    setRecvBufferSize() {}
    setSendBufferSize() {}
    getRecvBufferSize() { return 65536; }
    getSendBufferSize() { return 65536; }
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
