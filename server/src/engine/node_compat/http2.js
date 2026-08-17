// node:http2 — client subset over the policy-gated http2 ops, sized for
// stock gRPC clients (@grpc/grpc-js). Server-side APIs are not provided.
//
// The transport lives host-side (server/src/engine/http2.rs): sessions are
// policy-gated at connect, every stream is policy-gated and gets fetch
// header-injection rules applied at open, so gRPC credentials configured
// via --fetch-header never exist inside the isolate.
//
// Deliberate divergences from Node:
// - `options.createConnection` / custom sockets are ignored — the host owns
//   the transport (that is the security model). `session.socket` is a stub.
// - Only client sessions exist; `createServer` and push streams throw.

import { EventEmitter } from 'node:events';
import { Buffer } from 'node:buffer';

function ops() {
    const bound = globalThis.__mcpV8Http2Ops;
    if (!bound) {
        throw new Error(
            'http2 is not enabled: start mcp-v8 with an "http2" section in --policies-json');
    }
    return bound;
}

// ── base64 <-> bytes (op payloads are base64) ───────────────────────────

function b64FromBytes(bytes) {
    let bin = '';
    const CHUNK = 0x8000;
    for (let i = 0; i < bytes.length; i += CHUNK) {
        bin += String.fromCharCode.apply(null, bytes.subarray(i, i + CHUNK));
    }
    return btoa(bin);
}

function bytesFromB64(b64) {
    const bin = atob(b64 || '');
    const out = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i) & 0xff;
    return out;
}

// ── constants (the tables grpc-js reads) ────────────────────────────────

export const sensitiveHeaders = Symbol('nodejs.http2.sensitiveHeaders');

export const constants = Object.freeze({
    NGHTTP2_FLAG_NONE: 0,
    NGHTTP2_FLAG_END_STREAM: 1,
    NGHTTP2_FLAG_ACK: 1,
    NGHTTP2_FLAG_END_HEADERS: 4,
    NGHTTP2_FLAG_PADDED: 8,
    NGHTTP2_FLAG_PRIORITY: 32,

    NGHTTP2_NO_ERROR: 0,
    NGHTTP2_PROTOCOL_ERROR: 1,
    NGHTTP2_INTERNAL_ERROR: 2,
    NGHTTP2_FLOW_CONTROL_ERROR: 3,
    NGHTTP2_SETTINGS_TIMEOUT: 4,
    NGHTTP2_STREAM_CLOSED: 5,
    NGHTTP2_FRAME_SIZE_ERROR: 6,
    NGHTTP2_REFUSED_STREAM: 7,
    NGHTTP2_CANCEL: 8,
    NGHTTP2_COMPRESSION_ERROR: 9,
    NGHTTP2_CONNECT_ERROR: 10,
    NGHTTP2_ENHANCE_YOUR_CALM: 11,
    NGHTTP2_INADEQUATE_SECURITY: 12,
    NGHTTP2_HTTP_1_1_REQUIRED: 13,

    HTTP2_HEADER_STATUS: ':status',
    HTTP2_HEADER_METHOD: ':method',
    HTTP2_HEADER_AUTHORITY: ':authority',
    HTTP2_HEADER_SCHEME: ':scheme',
    HTTP2_HEADER_PATH: ':path',
    HTTP2_HEADER_CONTENT_TYPE: 'content-type',
    HTTP2_HEADER_CONTENT_LENGTH: 'content-length',
    HTTP2_HEADER_ACCEPT_ENCODING: 'accept-encoding',
    HTTP2_HEADER_TE: 'te',
    HTTP2_HEADER_USER_AGENT: 'user-agent',

    HTTP2_METHOD_POST: 'POST',
    HTTP2_METHOD_GET: 'GET',
});

// ── ClientHttp2Stream ───────────────────────────────────────────────────

class ClientHttp2Stream extends EventEmitter {
    constructor(session) {
        super();
        this._session = session;
        this._rid = null;
        this._pendingOpen = null; // resolves when the op-side stream exists
        this._writeChain = Promise.resolve();
        this._writableEnded = false;
        this._closed = false;
        this._paused = false;
        this._readQueue = [];
        this._readLoopDone = false;
        this.rstCode = undefined;
        this.id = undefined;
    }

    // Node Duplex subset. Data is sent in write order via a promise chain so
    // frames never interleave.
    write(chunk, encoding, callback) {
        if (typeof encoding === 'function') {
            callback = encoding;
            encoding = undefined;
        }
        if (this._writableEnded || this._closed) {
            // Node keeps the write side usable after the peer finishes its
            // response (e.g. a gRPC trailers-only reply): late writes are
            // discarded, not errors — the call's outcome comes from events.
            if (callback) callback();
            return true;
        }
        const bytes = toBytes(chunk, encoding);
        const self = this;
        this._writeChain = this._writeChain
            .then(() => self._opened())
            .then(() => ops().sendData(self._rid, b64FromBytes(bytes), false))
            .then(
                () => { if (callback) callback(); },
                (err) => {
                    // Same reasoning at the transport level: if the peer
                    // already concluded the stream, the failed write is
                    // moot; otherwise surface it.
                    if (self._closed) { if (callback) callback(); return; }
                    if (callback) callback(err);
                    else self._fail(err);
                });
        return true;
    }

    end(chunk, encoding, callback) {
        if (typeof chunk === 'function') { callback = chunk; chunk = undefined; }
        if (typeof encoding === 'function') { callback = encoding; encoding = undefined; }
        if (this._writableEnded || this._closed) {
            if (callback) callback();
            return this;
        }
        if (chunk !== undefined && chunk !== null) this.write(chunk, encoding);
        this._writableEnded = true;
        const self = this;
        this._writeChain = this._writeChain
            .then(() => self._opened())
            .then(() => ops().sendData(self._rid, '', true))
            .then(
                () => { if (callback) callback(); },
                (_err) => {
                    // A failed half-close carries no user data. The common
                    // cause is the peer completing the RPC (trailers or
                    // reset) before our empty END_STREAM frame — the read
                    // side delivers the call's real outcome either way, so
                    // this must not surface as a client error.
                    if (callback) callback();
                });
        return this;
    }

    get destroyed() { return this._closed; }

    pause() { this._paused = true; return this; }

    resume() {
        this._paused = false;
        while (!this._paused && this._readQueue.length > 0) {
            this.emit('data', this._readQueue.shift());
        }
        return this;
    }

    // http2stream.close(code): send RST_STREAM and finish locally.
    close(code, callback) {
        if (this._closed) {
            if (callback) callback();
            return;
        }
        this.rstCode = code === undefined ? constants.NGHTTP2_NO_ERROR : code;
        const self = this;
        this._opened().then(
            () => {
                if (self._rid !== null) {
                    try { ops().cancelStream(self._rid, self.rstCode); } catch (_e) {}
                }
                self._finishClose();
                if (callback) callback();
            },
            () => { self._finishClose(); if (callback) callback(); });
    }

    destroy(_error) { this.close(constants.NGHTTP2_CANCEL); }

    setTimeout(_ms, _cb) { /* not implemented; execution timeout governs */ }

    _opened() {
        return this._pendingOpen || Promise.reject(new Error('stream not started'));
    }

    _finishClose() {
        if (this._closed) return;
        this._closed = true;
        this.emit('close');
    }

    _fail(err) {
        if (this._closed) return;
        if (this.rstCode === undefined) this.rstCode = constants.NGHTTP2_INTERNAL_ERROR;
        this.emit('error', err instanceof Error ? err : new Error(String(err)));
        this._finishClose();
    }

    _deliverData(bytes) {
        const buf = Buffer.from(bytes);
        if (this._paused) this._readQueue.push(buf);
        else this.emit('data', buf);
    }

    async _run() {
        try {
            await this._pendingOpen;
        } catch (err) {
            this._fail(err);
            return;
        }

        try {
            const response = JSON.parse(await ops().response(this._rid));
            if (response.kind === 'reset') {
                this.rstCode = response.code;
                this._drop();
                const err = new Error('Stream closed with error code ' + response.code);
                err.code = 'ERR_HTTP2_STREAM_ERROR';
                this.emit('error', err);
                this._finishClose();
                return;
            }
            if (response.kind === 'error') {
                this._drop();
                this._fail(new Error(response.message || 'http2 stream error'));
                return;
            }
            const headers = response.headers || {};
            headers[':status'] = response.status;
            // The flags argument carries NGHTTP2_FLAG_END_STREAM for
            // trailers-only responses — consumers (grpc-js) read the gRPC
            // status from the headers when that bit is set.
            this.emit('response', headers, response.endStream ? constants.NGHTTP2_FLAG_END_STREAM : 0);
            if (response.endStream) {
                if (this.rstCode === undefined) this.rstCode = constants.NGHTTP2_NO_ERROR;
                this.emit('end');
                this._drop();
                this._finishClose();
                return;
            }

            for (;;) {
                const event = JSON.parse(await ops().read(this._rid));
                if (event.kind === 'data') {
                    this._deliverData(bytesFromB64(event.data));
                } else if (event.kind === 'trailers') {
                    this.emit('trailers', event.trailers || {}, 0);
                } else if (event.kind === 'end') {
                    if (this.rstCode === undefined) this.rstCode = constants.NGHTTP2_NO_ERROR;
                    this.emit('end');
                    this._drop();
                    this._finishClose();
                    return;
                } else if (event.kind === 'reset') {
                    this.rstCode = event.code;
                    this._drop();
                    const err = new Error('Stream closed with error code ' + event.code);
                    err.code = 'ERR_HTTP2_STREAM_ERROR';
                    this.emit('error', err);
                    this._finishClose();
                    return;
                } else { // "error"
                    this._drop();
                    this._fail(new Error(event.message || 'http2 stream error'));
                    return;
                }
            }
        } catch (err) {
            if (!this._closed) {
                this._drop();
                this._fail(err);
            }
        }
    }

    _drop() {
        if (this._rid !== null) {
            try { ops().dropStream(this._rid); } catch (_e) {}
        }
    }
}

function toBytes(chunk, encoding) {
    if (chunk instanceof Uint8Array) return chunk;
    if (chunk instanceof ArrayBuffer) return new Uint8Array(chunk);
    if (ArrayBuffer.isView(chunk)) {
        return new Uint8Array(chunk.buffer, chunk.byteOffset, chunk.byteLength);
    }
    return new Uint8Array(Buffer.from(String(chunk), encoding || 'utf8'));
}

// ── ClientHttp2Session ──────────────────────────────────────────────────

class ClientHttp2Session extends EventEmitter {
    constructor(authority, _options) {
        super();
        this._rid = null;
        this._closed = false;
        this._connectPromise = null;
        this.closed = false;
        this.destroyed = false;

        const url = typeof authority === 'string' ? authority : String(authority);
        const host = url.replace(/^https?:\/\//, '').replace(/\/.*$/, '');
        // Stub socket for channelz-style introspection and event attachment;
        // the real transport is host-side and its peer address is not
        // exposed to the isolate. It is an EventEmitter because consumers
        // (e.g. @grpc/grpc-js) attach close/error handlers to it.
        const socket = new EventEmitter();
        socket.remoteAddress = host.replace(/:\d+$/, '');
        socket.remotePort = Number((host.match(/:(\d+)$/) || [])[1]) || undefined;
        socket.localAddress = undefined;
        socket.localPort = undefined;
        socket.destroyed = false;
        socket.setNoDelay = () => socket;
        socket.setKeepAlive = () => socket;
        socket.setTimeout = () => socket;
        socket.ref = () => socket;
        socket.unref = () => socket;
        socket.destroy = () => socket;
        this.socket = socket;
        this.encrypted = /^https:/.test(url);

        const self = this;
        this._connectPromise = (async () => {
            const raw = await ops().connect(url);
            self._rid = JSON.parse(raw).rid;
            self.emit('connect', self);
        })();
        this._connectPromise.catch((err) => {
            self.destroyed = true;
            self.emit('error', err instanceof Error ? err : new Error(String(err)));
            self._finishClose();
        });
    }

    request(headers, options) {
        if (this._closed) throw new Error('session is closed');
        const map = {};
        for (const name of Object.keys(headers || {})) {
            // Symbol keys (sensitiveHeaders) are naturally excluded here.
            const value = headers[name];
            if (value === undefined || value === null) continue;
            map[name] = Array.isArray(value) ? value.join(', ') : String(value);
        }
        const endStream = !!(options && options.endStream);
        const stream = new ClientHttp2Stream(this);
        const self = this;
        // Node allows request() (and writes) immediately after connect();
        // the open promise is created eagerly so write()/end() can chain on
        // it, and resolves once the session handshake and the op-side open
        // (policy + header injection) complete.
        stream._pendingOpen = this._connectPromise.then(async () => {
            const raw = await ops().request(self._rid, JSON.stringify(map), endStream);
            const result = JSON.parse(raw);
            stream._rid = result.rid;
            stream.id = result.rid;
        });
        // _run and the write chain both consume the promise; this guard
        // keeps an early failure from being an unhandled rejection.
        stream._pendingOpen.catch(() => {});
        stream._run();
        return stream;
    }

    // Flow-control introspection (consumers log these; the real windows are
    // managed host-side by the h2 crate). Static defaults are reported.
    get state() {
        return {
            effectiveLocalWindowSize: 65535,
            effectiveRecvDataLength: 0,
            nextStreamID: 1,
            localWindowSize: 65535,
            lastProcStreamID: 0,
            remoteWindowSize: 65535,
            outboundQueueSize: 0,
            deflateDynamicTableSize: 4096,
            inflateDynamicTableSize: 4096,
        };
    }

    ping(payloadOrCallback, callback) {
        const cb = typeof payloadOrCallback === 'function' ? payloadOrCallback : callback;
        const self = this;
        this._connectPromise
            .then(() => ops().ping(self._rid))
            .then(
                () => { if (cb) cb(null, 0, Buffer.alloc(8)); },
                (err) => { if (cb) cb(err instanceof Error ? err : new Error(String(err)), 0, Buffer.alloc(8)); });
        return true;
    }

    close(callback) {
        if (callback) this.once('close', callback);
        this._teardown();
    }

    destroy(_error, _code) {
        this.destroyed = true;
        this._teardown();
    }

    ref() {}
    unref() {}
    setTimeout(_ms, _cb) {}

    _teardown() {
        if (this._closed) return;
        const self = this;
        this._connectPromise.then(
            () => {
                if (self._rid !== null) {
                    try { ops().closeSession(self._rid); } catch (_e) {}
                }
                self._finishClose();
            },
            () => self._finishClose());
    }

    _finishClose() {
        if (this._closed) return;
        this._closed = true;
        this.closed = true;
        this.destroyed = true;
        this.socket.destroyed = true;
        this.emit('close');
    }
}

// ── module surface ──────────────────────────────────────────────────────

export function connect(authority, options, listener) {
    if (typeof options === 'function') {
        listener = options;
        options = undefined;
    }
    const session = new ClientHttp2Session(authority, options);
    if (listener) session.once('connect', listener);
    return session;
}

export function createServer() {
    throw new Error('node:http2 server APIs are not supported in this runtime');
}

export function createSecureServer() {
    throw new Error('node:http2 server APIs are not supported in this runtime');
}

export function getDefaultSettings() {
    return {
        headerTableSize: 4096,
        enablePush: false,
        initialWindowSize: 65535,
        maxFrameSize: 16384,
        maxConcurrentStreams: 4294967295,
        maxHeaderListSize: 65535,
    };
}

export default {
    connect,
    constants,
    sensitiveHeaders,
    createServer,
    createSecureServer,
    getDefaultSettings,
};
