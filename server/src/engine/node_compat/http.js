// node:http — HTTP/1.1 server and client over the loopback-only node:net
// capability. When the host has not enabled loopback TCP (the default
// sandbox), createServer/request throw the same capability-model errors as
// before and fetch() remains the supported HTTP client. The implementation
// targets the patterns the Node test corpus and small libraries use:
// keep-alive with sequential requests, Content-Length and chunked framing
// both ways, implicit chunked responses, HEAD/204/304 body suppression.

import { EventEmitter } from 'node:events';
import { Readable, Writable } from 'node:stream';
import { Buffer } from 'node:buffer';
import net from 'node:net';

const netEnabled = Boolean(globalThis.__mcpV8NetOps);

// Node's http constructors predate class syntax and are callable without
// `new`; a Proxy apply trap preserves that while keeping instanceof and
// prototype identity.
function callable(Cls) {
    return new Proxy(Cls, {
        apply: (target, _thisArg, args) => new target(...args),
    });
}

export const METHODS = [
    'ACL', 'BIND', 'CHECKOUT', 'CONNECT', 'COPY', 'DELETE', 'GET', 'HEAD',
    'LINK', 'LOCK', 'M-SEARCH', 'MERGE', 'MKACTIVITY', 'MKCALENDAR', 'MKCOL',
    'MOVE', 'NOTIFY', 'OPTIONS', 'PATCH', 'POST', 'PROPFIND', 'PROPPATCH',
    'PURGE', 'PUT', 'QUERY', 'REBIND', 'REPORT', 'SEARCH', 'SOURCE',
    'SUBSCRIBE', 'TRACE', 'UNBIND', 'UNLINK', 'UNLOCK', 'UNSUBSCRIBE',
];

export const STATUS_CODES = {
    100: 'Continue', 101: 'Switching Protocols', 102: 'Processing',
    103: 'Early Hints', 200: 'OK', 201: 'Created', 202: 'Accepted',
    203: 'Non-Authoritative Information', 204: 'No Content',
    205: 'Reset Content', 206: 'Partial Content', 207: 'Multi-Status',
    208: 'Already Reported', 226: 'IM Used', 300: 'Multiple Choices',
    301: 'Moved Permanently', 302: 'Found', 303: 'See Other',
    304: 'Not Modified', 305: 'Use Proxy', 307: 'Temporary Redirect',
    308: 'Permanent Redirect', 400: 'Bad Request', 401: 'Unauthorized',
    402: 'Payment Required', 403: 'Forbidden', 404: 'Not Found',
    405: 'Method Not Allowed', 406: 'Not Acceptable',
    407: 'Proxy Authentication Required', 408: 'Request Timeout',
    409: 'Conflict', 410: 'Gone', 411: 'Length Required',
    412: 'Precondition Failed', 413: 'Payload Too Large', 414: 'URI Too Long',
    415: 'Unsupported Media Type', 416: 'Range Not Satisfiable',
    417: 'Expectation Failed', 418: "I'm a Teapot",
    421: 'Misdirected Request', 422: 'Unprocessable Entity', 423: 'Locked',
    424: 'Failed Dependency', 425: 'Too Early', 426: 'Upgrade Required',
    428: 'Precondition Required', 429: 'Too Many Requests',
    431: 'Request Header Fields Too Large', 451: 'Unavailable For Legal Reasons',
    500: 'Internal Server Error', 501: 'Not Implemented', 502: 'Bad Gateway',
    503: 'Service Unavailable', 504: 'Gateway Timeout',
    505: 'HTTP Version Not Supported', 506: 'Variant Also Negotiates',
    507: 'Insufficient Storage', 508: 'Loop Detected',
    509: 'Bandwidth Limit Exceeded', 510: 'Not Extended',
    511: 'Network Authentication Required',
};

export const maxHeaderSize = 16384;

const TOKEN_RE = /^[\^_`a-zA-Z\-0-9!#$%&'*+.|~]+$/;
// Anything outside 0x21-0xff must be escaped in a request path.
const INVALID_PATH_RE = /[^!-ÿ]/;

function receivedRepr(value) {
    if (value === undefined) return 'undefined';
    if (value === null) return 'null';
    const type = typeof value;
    if (type === 'string') return `type string ('${value}')`;
    if (type === 'object') {
        const name = value.constructor && value.constructor.name;
        return `an instance of ${name || 'Object'}`;
    }
    if (type === 'function') return `function ${value.name}`;
    if (type === 'bigint') return `type bigint (${value}n)`;
    return `type ${type} (${String(value)})`;
}

function argTypeError(message) {
    const err = new TypeError(message);
    err.code = 'ERR_INVALID_ARG_TYPE';
    return err;
}

function validateRequestOptions(opts) {
    if (opts.method !== undefined && opts.method !== null) {
        if (typeof opts.method !== 'string') {
            throw argTypeError(
                `The "options.method" property must be of type string. Received ${receivedRepr(opts.method)}`);
        }
        if (!TOKEN_RE.test(opts.method)) {
            const err = new TypeError(`Method must be a valid HTTP token ["${opts.method}"]`);
            err.code = 'ERR_INVALID_HTTP_TOKEN';
            throw err;
        }
    }
    if (opts.path !== undefined && INVALID_PATH_RE.test(String(opts.path))) {
        const err = new TypeError('Request path contains unescaped characters');
        err.code = 'ERR_UNESCAPED_CHARACTERS';
        throw err;
    }
    const agent = opts.agent;
    if (agent !== undefined && agent !== null && agent !== false
        && !(typeof agent === 'object' && typeof agent.addRequest === 'function')) {
        throw argTypeError(
            'The "options.agent" property must be one of Agent-like Object, ' +
            `undefined, or false. Received ${receivedRepr(agent)}`);
    }
    if (opts.insecureHTTPParser !== undefined
        && typeof opts.insecureHTTPParser !== 'boolean') {
        throw argTypeError(
            `The "options.insecureHTTPParser" property must be of type boolean. Received ${receivedRepr(opts.insecureHTTPParser)}`);
    }
    if (opts.timeout !== undefined && typeof opts.timeout !== 'number') {
        throw argTypeError(
            `The "timeout" argument must be of type number. Received ${receivedRepr(opts.timeout)}`);
    }
    if (opts.headers && Array.isArray(opts.headers.host)) {
        throw argTypeError(
            `The "options.headers.host" property must be of type string. Received ${receivedRepr(opts.headers.host)}`);
    }
}

// A body chunk written to an OutgoingMessage must be a string or a
// Buffer/TypedArray/DataView; anything else (an Array, an object) is
// ERR_INVALID_ARG_TYPE, before the stream layer sees it.
function validateOutgoingChunk(chunk) {
    if (chunk === undefined || chunk === null) return;
    if (typeof chunk === 'string' || ArrayBuffer.isView(chunk)) return;
    const err = new TypeError(
        'The "chunk" argument must be of type string or an instance of ' +
        `Buffer or Uint8Array. Received ${receivedRepr(chunk)}`);
    err.code = 'ERR_INVALID_ARG_TYPE';
    throw err;
}

export function validateHeaderName(name) {
    if (typeof name !== 'string' || !name || !TOKEN_RE.test(name)) {
        const err = new TypeError(
            `Header name must be a valid HTTP token ["${name}"]`);
        err.code = 'ERR_INVALID_HTTP_TOKEN';
        throw err;
    }
}

export function validateHeaderValue(name, value) {
    if (value === undefined) {
        const err = new TypeError(`Invalid value "${value}" for header "${name}"`);
        err.code = 'ERR_HTTP_INVALID_HEADER_VALUE';
        throw err;
    }
    // Node's checkInvalidHeaderChar: valid field-value bytes are tab, the
    // printable ASCII range, and the Latin-1 high range. Anything else
    // (CR/LF injection, other control chars, non-Latin-1 code points) is
    // ERR_INVALID_CHAR.
    if (/[^\t\x20-\x7e\x80-\xff]/.test(String(value))) {
        const err = new TypeError(
            `Invalid character in header content ["${name}"]`);
        err.code = 'ERR_INVALID_CHAR';
        throw err;
    }
}

// ── header collection helpers ───────────────────────────────────────────

// Headers Node treats as single-valued: duplicates are discarded rather
// than joined.
const SINGLETON_HEADERS = new Set([
    'age', 'authorization', 'content-length', 'content-type', 'etag',
    'expires', 'from', 'host', 'if-modified-since', 'if-unmodified-since',
    'last-modified', 'location', 'max-forwards', 'proxy-authorization',
    'referer', 'retry-after', 'server', 'user-agent',
]);

function addIncomingHeader(headers, rawHeaders, name, value, joinDuplicates) {
    rawHeaders.push(name, value);
    const key = name.toLowerCase();
    const existing = Object.hasOwn(headers, key) ? headers[key] : undefined;
    if (existing !== undefined && SINGLETON_HEADERS.has(key)) {
        if (joinDuplicates && key !== 'set-cookie' && key !== 'cookie') {
            headers[key] = existing + ', ' + value;
        }
        return;
    }
    if (existing === undefined) {
        Object.defineProperty(headers, key, {
            value: key === 'set-cookie' ? [value] : value,
            writable: true, enumerable: true, configurable: true,
        });
        return;
    }
    if (key === 'set-cookie') {
        existing.push(value);
    } else if (key === 'cookie') {
        headers[key] = existing + '; ' + value;
    } else {
        headers[key] = existing + ', ' + value;
    }
}

// ── incoming message ────────────────────────────────────────────────────

class IncomingMessageImpl extends Readable {
    constructor(socket) {
        super({});
        this.socket = socket;
        this.connection = socket;
        this.httpVersion = '1.1';
        this.httpVersionMajor = 1;
        this.httpVersionMinor = 1;
        this.headers = {};
        this.rawHeaders = [];
        this.trailers = {};
        this.rawTrailers = [];
        this.method = null;
        this.url = '';
        this.statusCode = null;
        this.statusMessage = null;
        this.complete = false;
        this.aborted = false;
    }

    _read() {}

    setTimeout(ms, callback) {
        if (this.socket) this.socket.setTimeout(ms, callback);
        return this;
    }

    destroy(err) {
        // The error belongs to this message; tearing the socket down with it
        // would rebroadcast it through socket 'error' forwarding.
        if (this.socket && !this.socket.destroyed) this.socket.destroy();
        return super.destroy(err);
    }
}

// ── outgoing message (shared by ServerResponse / ClientRequest) ─────────

class OutgoingMessageImpl extends Writable {
    constructor() {
        super({});
        this.headersSent = false;
        this.finished = false;
        this.socket = null;
        this.connection = null;
        this._headers = new Map();  // lower-name -> [name, value]
        this._chunked = false;
        this._suppressBody = false;
    }

    setHeader(name, value) {
        validateHeaderName(name);
        validateHeaderValue(name, value);
        if (this.headersSent) {
            const err = new Error('Cannot set headers after they are sent to the client');
            err.code = 'ERR_HTTP_HEADERS_SENT';
            throw err;
        }
        this._headers.set(String(name).toLowerCase(), [String(name), value]);
        return this;
    }

    getHeader(name) {
        const entry = this._headers.get(String(name).toLowerCase());
        return entry ? entry[1] : undefined;
    }

    getHeaders() {
        const out = Object.create(null);
        for (const [key, [, value]] of this._headers) out[key] = value;
        return out;
    }

    getHeaderNames() {
        return [...this._headers.keys()];
    }

    hasHeader(name) {
        return this._headers.has(String(name).toLowerCase());
    }

    removeHeader(name) {
        if (this.headersSent) {
            const err = new Error('Cannot remove headers after they are sent to the client');
            err.code = 'ERR_HTTP_HEADERS_SENT';
            throw err;
        }
        this._headers.delete(String(name).toLowerCase());
    }

    appendHeader(name, value) {
        const key = String(name).toLowerCase();
        const entry = this._headers.get(key);
        if (!entry) return this.setHeader(name, value);
        const merged = Array.isArray(entry[1])
            ? entry[1].concat(value)
            : [entry[1]].concat(value);
        this._headers.set(key, [entry[0], merged]);
        return this;
    }

    flushHeaders() {
        this._flushHead();
    }

    addTrailers(trailers) {
        this._trailers = trailers;
    }

    setTimeout(ms, callback) {
        if (this.socket) this.socket.setTimeout(ms, callback);
        return this;
    }

    clearTimeout(callback) {
        if (this.socket) this.socket.setTimeout(0, callback);
        return this;
    }

    _serializeHeaders() {
        let out = '';
        for (const [, [name, value]] of this._headers) {
            for (const v of Array.isArray(value) ? value : [value]) {
                out += `${name}: ${v}\r\n`;
            }
        }
        return out;
    }

    _writeRaw(data) {
        if (this.socket && !this.socket.destroyed) {
            // Transport failures (peer already gone: EPIPE/ECONNRESET)
            // surface through 'aborted'/'clientError', not the write path.
            this.socket.write(data, (error) => void error);
        }
    }

    write(chunk, encoding, callback) {
        validateOutgoingChunk(chunk);
        return super.write(chunk, encoding, callback);
    }

    _write(chunk, encoding, callback) {
        this._wroteBody = true;
        this._flushHead();
        const buf = Buffer.isBuffer(chunk)
            ? chunk
            : Buffer.from(String(chunk), encoding || 'utf8');
        if (this._suppressBody || buf.length === 0) { callback(); return; }
        if (this._chunked) {
            this._writeRaw(Buffer.concat([
                Buffer.from(buf.length.toString(16) + '\r\n'),
                buf,
                Buffer.from('\r\n'),
            ]));
        } else {
            this._writeRaw(buf);
        }
        callback();
    }

    _final(callback) {
        this._flushHead();
        if (this._chunked && !this._suppressBody) {
            let block = '0\r\n';
            if (this._trailers) {
                const entries = Array.isArray(this._trailers)
                    ? this._trailers
                    : Object.entries(this._trailers);
                for (const [name, value] of entries) {
                    block += `${name}: ${value}\r\n`;
                }
            }
            this._writeRaw(Buffer.from(block + '\r\n'));
        }
        this.finished = true;
        this._afterFinal();
        callback();
    }

    _flushHead() {}
    _afterFinal() {}
}

// ── server response ─────────────────────────────────────────────────────

class ServerResponseImpl extends OutgoingMessageImpl {
    constructor(req) {
        super();
        this.req = req;
        this.socket = req.socket;
        this.connection = req.socket;
        this.statusCode = 200;
        this.statusMessage = undefined;
        this.sendDate = true;
        this._keepAlive = shouldKeepAlive(req);
        this._suppressBody = req.method === 'HEAD';
    }

    writeHead(statusCode, reason, headers) {
        if (typeof reason === 'object' && reason !== null) {
            headers = reason;
            reason = undefined;
        }
        this.statusCode = statusCode;
        if (reason !== undefined) this.statusMessage = reason;
        // A CR/LF in the reason phrase is a header-injection vector; Node
        // rejects it here (explicit) and at flush time (implicit setter).
        if (this.statusMessage !== undefined && this.statusMessage !== null
            && /[\r\n]/.test(String(this.statusMessage))) {
            throw new Error('Invalid character in statusMessage');
        }
        if (headers) {
            if (Array.isArray(headers)) {
                if (headers.length > 0 && Array.isArray(headers[0])) {
                    for (const [name, value] of headers) {
                        this.appendHeader(name, value);
                    }
                } else {
                    for (let i = 0; i + 1 < headers.length; i += 2) {
                        this.appendHeader(headers[i], headers[i + 1]);
                    }
                }
            } else {
                for (const name of Object.keys(headers)) {
                    this.setHeader(name, headers[name]);
                }
            }
        }
        this._flushHead();
        return this;
    }

    // Node buffers a single-shot end() and frames it with Content-Length
    // rather than chunked encoding.
    end(chunk, encoding, callback) {
        if (typeof chunk !== 'function') validateOutgoingChunk(chunk);
        if (!this.headersSent && !this._wroteBody && !this._suppressBody
            && !this.hasHeader('trailer')
            && !this.hasHeader('content-length') && !this.hasHeader('transfer-encoding')) {
            const length = chunk == null ? 0 : (Buffer.isBuffer(chunk)
                ? chunk.length
                : Buffer.byteLength(String(chunk), typeof encoding === 'string' ? encoding : 'utf8'));
            this.setHeaderInternal('Content-Length', String(length));
        }
        return super.end(chunk, encoding, callback);
    }

    writeContinue(callback) {
        this._writeRaw('HTTP/1.1 100 Continue\r\n\r\n');
        if (typeof callback === 'function') Promise.resolve().then(callback);
    }

    writeProcessing(callback) {
        this._writeRaw('HTTP/1.1 102 Processing\r\n\r\n');
        if (typeof callback === 'function') Promise.resolve().then(callback);
    }

    // Send an interim 1xx informational response (RFC 8297 / 100-series).
    // 101 is reserved for protocol upgrade and is rejected here.
    writeInformation(code, headers, callback) {
        if (typeof headers === 'function') { callback = headers; headers = undefined; }
        if (typeof code !== 'number') {
            const err = new TypeError(
                `The "code" argument must be of type number. Received ${receivedRepr(code)}`);
            err.code = 'ERR_INVALID_ARG_TYPE';
            throw err;
        }
        if (code === 101) {
            const err = new RangeError(`Invalid status code: ${code}`);
            err.code = 'ERR_HTTP_INVALID_STATUS_CODE';
            throw err;
        }
        if (!Number.isInteger(code) || code < 100 || code > 199) {
            const err = new RangeError(
                `The value of "code" is out of range. It must be >= 100 && <= 199. Received ${code}`);
            err.code = 'ERR_OUT_OF_RANGE';
            throw err;
        }
        if (this.headersSent) {
            const err = new Error('Cannot render headers after they are sent to the client');
            err.code = 'ERR_HTTP_HEADERS_SENT';
            throw err;
        }
        let block = `HTTP/1.1 ${code} ${STATUS_CODES[code] || 'unknown'}\r\n`;
        const emit = (name, value) => {
            validateHeaderName(name);
            validateHeaderValue(name, value);
            block += `${name}: ${value}\r\n`;
        };
        if (Array.isArray(headers)) {
            if (headers.length > 0 && Array.isArray(headers[0])) {
                for (const [name, value] of headers) emit(name, value);
            } else {
                for (let i = 0; i + 1 < headers.length; i += 2) emit(headers[i], headers[i + 1]);
            }
        } else if (headers && typeof headers === 'object') {
            for (const name of Object.keys(headers)) {
                const value = headers[name];
                if (Array.isArray(value)) for (const v of value) emit(name, v);
                else emit(name, value);
            }
        }
        this._writeRaw(block + '\r\n');
        if (typeof callback === 'function') Promise.resolve().then(callback);
    }

    // Early Hints (103): a convenience wrapper over writeInformation for the
    // Link header preload pattern. `hints.link` (a string or array of link
    // values) becomes a single Link header; an empty set sends nothing.
    writeEarlyHints(hints, callback) {
        const headers = {};
        let hasHeader = false;
        if (hints && typeof hints === 'object') {
            for (const name of Object.keys(hints)) {
                if (name.toLowerCase() === 'link') continue;
                headers[name] = hints[name];
                hasHeader = true;
            }
            if (hints.link !== undefined) {
                const link = Array.isArray(hints.link)
                    ? hints.link.join(', ') : String(hints.link);
                if (link.length > 0) { headers.Link = link; hasHeader = true; }
            }
        }
        if (!hasHeader) {
            if (typeof callback === 'function') Promise.resolve().then(callback);
            return;
        }
        this.writeInformation(103, headers, callback);
    }


    _flushHead() {
        if (this.headersSent) return;
        // Implicit writeHead (a statusMessage set directly then a body
        // write) validates the reason phrase here.
        if (this.statusMessage !== undefined && this.statusMessage !== null
            && /[\r\n]/.test(String(this.statusMessage))) {
            throw new Error('Invalid character in statusMessage');
        }
        this.headersSent = true;
        const status = this.statusCode;
        const message = this.statusMessage !== undefined
            ? this.statusMessage
            : (STATUS_CODES[status] || 'unknown');
        const noBody = this._suppressBody
            || status === 204 || status === 304 || (status >= 100 && status < 200);
        const useChunked = !this.hasHeader('content-length')
            && !this.hasHeader('transfer-encoding') && !noBody;
        // A body-framing header on a bodyless status makes the response
        // length ambiguous; Node answers by closing the connection.
        if ((status === 204 || status === 304)
            && (this.hasHeader('transfer-encoding') || this.hasHeader('content-length'))) {
            this._keepAlive = false;
        }
        if (this.sendDate && !this.hasHeader('date')) {
            this.setHeaderInternal('Date', new Date().toUTCString());
        }
        if (!this.hasHeader('connection')) {
            this.setHeaderInternal('Connection', this._keepAlive ? 'keep-alive' : 'close');
        } else if (String(this.getHeader('connection')).toLowerCase() === 'close') {
            this._keepAlive = false;
        }
        if (useChunked) {
            // Node appends Transfer-Encoding after Date/Connection.
            this._chunked = true;
            this.setHeaderInternal('Transfer-Encoding', 'chunked');
        } else if (!noBody
            && /chunked/i.test(String(this.getHeader('transfer-encoding') || ''))) {
            this._chunked = true;
        }
        this._writeRaw(
            `HTTP/1.1 ${status} ${message}\r\n` + this._serializeHeaders() + '\r\n');
        if (noBody) this._suppressBody = true;
    }

    // Internal header writes bypass the headersSent guard (used during flush).
    setHeaderInternal(name, value) {
        this._headers.set(String(name).toLowerCase(), [String(name), value]);
    }

    _afterFinal() {
        const socket = this.socket;
        if (socket) socket._httpActive = false;
        if (socket && !socket.destroyed
            && (!this._keepAlive || (socket._server && socket._server._closing))) {
            socket.end();
        }
        Promise.resolve().then(() => this.emit('close'));
    }
}

function shouldKeepAlive(req) {
    const connection = String(req.headers.connection || '').toLowerCase();
    if (req.httpVersionMajor === 1 && req.httpVersionMinor === 0) {
        return connection.includes('keep-alive');
    }
    return !connection.includes('close');
}

// ── request parsing (server side) ───────────────────────────────────────

const EMPTY = Buffer.alloc(0);

class ConnectionParser {
    constructor(server, socket) {
        this.server = server;
        this.socket = socket;
        this.buf = EMPTY;
        this.req = null;          // message currently receiving its body
        this.remaining = null;    // content-length countdown
        this.chunked = false;
        this.chunkRemaining = null; // null = expecting a size line
        this.activeReq = null;    // request whose response is unfinished
        this.activeRes = null;
    }

    // Connection died: any request mid-body or with an unfinished response
    // learns about it the way Node reports it.
    onSocketClose() {
        const req = this.activeReq;
        const res = this.activeRes;
        this.activeReq = null;
        this.activeRes = null;
        this.req = null;
        if (req && (!req.complete || (res && !res.finished))) {
            req.aborted = true;
            req.emit('aborted');
            if (req.listenerCount('error') > 0) {
                const err = new Error('aborted');
                err.code = 'ECONNRESET';
                req.emit('error', err);
            }
            req.emit('close');
            if (res && !res.finished) {
                Promise.resolve().then(() => res.emit('close'));
            }
        }
    }

    feed(chunk) {
        // After an Upgrade/CONNECT the socket belongs to the user; stop
        // interpreting its bytes as HTTP.
        if (this.upgraded) return;
        if (!Buffer.isBuffer(chunk)) chunk = Buffer.from(chunk);
        this.buf = this.buf.length === 0 ? chunk : Buffer.concat([this.buf, chunk]);
        this.process();
    }

    process() {
        for (;;) {
            if (this.upgraded) return;
            if (this.req === null) {
                const headEnd = this.buf.indexOf('\r\n\r\n');
                if (headEnd === -1) return;
                const head = this.buf.subarray(0, headEnd).toString('latin1');
                this.buf = this.buf.subarray(headEnd + 4);
                if (!this.beginRequest(head)) return;
                continue;
            }
            if (this.chunked) {
                if (!this.processChunked()) return;
                continue;
            }
            if (this.remaining > 0) {
                if (this.buf.length === 0) return;
                const take = Math.min(this.remaining, this.buf.length);
                this.req.push(this.buf.subarray(0, take));
                this.buf = this.buf.subarray(take);
                this.remaining -= take;
            }
            if (this.remaining === 0) this.endBody();
            else return;
        }
    }

    beginRequest(head) {
        const lines = head.split('\r\n');
        const match = /^(\S+) (\S+) HTTP\/(\d)\.(\d)$/.exec(lines[0]);
        if (!match) {
            this.socket.end('HTTP/1.1 400 Bad Request\r\nConnection: close\r\n\r\n');
            return false;
        }
        this.socket._httpActive = true;
        const req = new IncomingMessageImpl(this.socket);
        req.method = match[1];
        req.url = match[2];
        req.httpVersionMajor = Number(match[3]);
        req.httpVersionMinor = Number(match[4]);
        req.httpVersion = `${match[3]}.${match[4]}`;
        for (let i = 1; i < lines.length; i++) {
            const sep = lines[i].indexOf(':');
            if (sep === -1) continue;
            addIncomingHeader(
                req.headers, req.rawHeaders,
                lines[i].slice(0, sep).trim(), lines[i].slice(sep + 1).trim(),
                Boolean(this.server._options && this.server._options.joinDuplicateHeaders));
        }
        // HTTP Upgrade / CONNECT: when the server has a matching listener,
        // hand it the raw socket (req, socket, head) and detach from HTTP
        // parsing — no 'request' event, no ServerResponse. Without a
        // listener the request is served normally, like Node.
        const upgradeHeader = req.headers['upgrade'];
        const connUpgrade = String(req.headers['connection'] || '')
            .toLowerCase().split(',').some((t) => t.trim() === 'upgrade');
        const isConnect = req.method === 'CONNECT';
        // A shouldUpgradeCallback option can veto an upgrade, in which case
        // the request is served normally (Node 22+).
        const shouldUpgrade = this.server._options
            && typeof this.server._options.shouldUpgradeCallback === 'function'
            ? Boolean(this.server._options.shouldUpgradeCallback(req))
            : true;
        if (shouldUpgrade
            && ((isConnect && this.server.listenerCount('connect') > 0)
                || (upgradeHeader !== undefined && connUpgrade
                    && this.server.listenerCount('upgrade') > 0))) {
            const head = this.buf;
            this.buf = EMPTY;
            this.req = null;
            this.upgraded = true;
            req.complete = true;
            if (this.socket) this.socket._httpUpgraded = true;
            try {
                this.server.emit(isConnect ? 'connect' : 'upgrade',
                    req, this.socket, head);
            } catch (error) {
                // A throw from an upgrade/connect handler is an uncaught
                // exception in Node, never a socket teardown.
                if (error && typeof error === 'object') {
                    error.__fromRequestHandler = true;
                }
                throw error;
            }
            return false;
        }
        const te = String(req.headers['transfer-encoding'] || '').toLowerCase();
        const contentLength = req.headers['content-length'];
        this.req = req;
        if (te.includes('chunked')) {
            this.chunked = true;
            this.chunkRemaining = null;
        } else {
            this.chunked = false;
            this.remaining = contentLength === undefined ? 0 : Number(contentLength);
            if (!Number.isFinite(this.remaining) || this.remaining < 0) this.remaining = 0;
        }
        let res;
        try {
            res = new ServerResponseImpl(req);
        } catch (error) {
            error.__fromRequestHandler = true;
            throw error;
        }
        this.activeReq = req;
        this.activeRes = res;
        res.on('finish', () => {
            if (this.activeRes === res) {
                this.activeReq = null;
                this.activeRes = null;
            }
        });
        try {
            if (req.headers.expect &&
                String(req.headers.expect).toLowerCase() === '100-continue') {
                if (this.server.listenerCount('checkContinue') > 0) {
                    this.server.emit('checkContinue', req, res);
                } else {
                    res.writeContinue();
                    this.server.emit('request', req, res);
                }
            } else {
                this.server.emit('request', req, res);
            }
        } catch (error) {
            // A throw from the user's request listener is an uncaught
            // exception in Node, never a connection teardown.
            if (error && typeof error === 'object') error.__fromRequestHandler = true;
            throw error;
        }
        return true;
    }

    processChunked() {
        for (;;) {
            if (this.chunkRemaining === null) {
                const lineEnd = this.buf.indexOf('\r\n');
                if (lineEnd === -1) return false;
                const sizeLine = this.buf.subarray(0, lineEnd).toString('latin1');
                this.buf = this.buf.subarray(lineEnd + 2);
                if (!/^[0-9a-fA-F]+(;.*)?$/.test(sizeLine)) {
                    this.socket.destroy(new Error('Parse Error: Invalid character in chunk size'));
                    return false;
                }
                const size = parseInt(sizeLine, 16);
                if (size === 0) { this.chunkRemaining = 0; }
                else this.chunkRemaining = size;
            }
            if (this.chunkRemaining === -1) {
                // Waiting for the CRLF that closes a data chunk.
                if (this.buf.length < 2) return false;
                this.buf = this.buf.subarray(2);
                this.chunkRemaining = null;
                continue;
            }
            if (this.chunkRemaining === 0) {
                // Trailer section: header lines until a blank line.
                for (;;) {
                    const lineEnd = this.buf.indexOf('\r\n');
                    if (lineEnd === -1) return false;
                    const line = this.buf.subarray(0, lineEnd).toString('latin1');
                    this.buf = this.buf.subarray(lineEnd + 2);
                    if (!line) { this.endBody(); return true; }
                    const sep = line.indexOf(':');
                    if (sep !== -1 && this.req) {
                        const name = line.slice(0, sep).trim();
                        const value = line.slice(sep + 1).trim();
                        this.req.rawTrailers.push(name, value);
                        const key = name.toLowerCase();
                        this.req.trailers[key] = Object.hasOwn(this.req.trailers, key)
                            ? this.req.trailers[key] + ', ' + value : value;
                    }
                }
            }
            if (this.buf.length === 0) return false;
            const take = Math.min(this.chunkRemaining, this.buf.length);
            this.req.push(this.buf.subarray(0, take));
            this.buf = this.buf.subarray(take);
            this.chunkRemaining -= take;
            if (this.chunkRemaining === 0) {
                if (this.buf.length < 2) { this.chunkRemaining = -1; return false; }
                this.buf = this.buf.subarray(2);
                this.chunkRemaining = null;
            }
        }
    }

    endBody() {
        const req = this.req;
        this.req = null;
        this.chunked = false;
        this.chunkRemaining = null;
        this.remaining = null;
        if (req) {
            req.complete = true;
            req.once('end', () => { req.destroyed = true; });
            req.push(null);
        }
    }
}

// ── server ──────────────────────────────────────────────────────────────

class ServerImpl extends net.Server {
    constructor(options, requestListener) {
        if (typeof options === 'function') {
            requestListener = options;
            options = {};
        }
        super(options || {});
        const opts = options || {};
        this.timeout = 0;
        this.keepAliveTimeout = opts.keepAliveTimeout !== undefined
            ? opts.keepAliveTimeout : 5000;
        this.headersTimeout = opts.headersTimeout !== undefined
            ? opts.headersTimeout : 60000;
        this.requestTimeout = opts.requestTimeout !== undefined
            ? opts.requestTimeout : 300000;
        this.maxHeadersCount = null;
        this.maxRequestsPerSocket = 0;
        if (typeof requestListener === 'function') {
            this.on('request', requestListener);
        }
        this.on('connection', (socket) => this._setupConnection(socket));
    }

    _setupConnection(socket) {
        // Server-level idle timeout applies to each connection; with no
        // 'timeout' listener Node destroys the socket.
        if (this.timeout > 0 && typeof socket.setTimeout === 'function') {
            socket.setTimeout(this.timeout);
        }
        socket.on('timeout', () => {
            if (this.listenerCount('timeout') > 0) {
                this.emit('timeout', socket);
            } else {
                socket.destroy();
            }
        });
        const parser = new ConnectionParser(this, socket);
        socket.on('data', (chunk) => {
            try {
                parser.feed(chunk);
            } catch (error) {
                if (error && error.__fromRequestHandler) {
                    delete error.__fromRequestHandler;
                    throw error;
                }
                socket.destroy(error);
            }
        });
        // Swallow connection teardown races; 'clientError' is the Node hook.
        socket.on('error', (error) => {
            if (this.listenerCount('clientError') > 0) {
                this.emit('clientError', error, socket);
            }
        });
        socket.on('close', () => parser.onSocketClose());
    }

    setTimeout(ms, callback) {
        this.timeout = ms;
        if (callback) this.on('timeout', callback);
        for (const socket of this._connections) {
            if (typeof socket.setTimeout === 'function') socket.setTimeout(ms);
        }
        return this;
    }

    // Node 19+: close() also closes idle keep-alive connections; sockets
    // mid-request drain first (_afterFinal ends them once the server is
    // closing).
    close(cb) {
        super.close(cb);
        this.closeIdleConnections();
        return this;
    }

    closeAllConnections() {
        for (const socket of [...this._connections]) socket.destroy();
    }

    closeIdleConnections() {
        for (const socket of [...this._connections]) {
            if (!socket._httpActive) socket.destroy();
        }
    }
}

export function createServer(options, requestListener) {
    if (!netEnabled) {
        throw new Error('http.createServer is not supported in this runtime');
    }
    return new ServerImpl(options, requestListener);
}

// ── client ──────────────────────────────────────────────────────────────

class AgentImpl extends EventEmitter {
    constructor(options) {
        super();
        this.options = options || {};
        this.keepAlive = Boolean(this.options.keepAlive);
        this.defaultPort = this.options.defaultPort || 80;
        this.protocol = this.options.protocol || 'http:';
        this.maxSockets = this.options.maxSockets || Infinity;
        this.maxFreeSockets = this.options.maxFreeSockets || 256;
        if (this.options.maxTotalSockets !== undefined) {
            const value = this.options.maxTotalSockets;
            if (typeof value !== 'number') {
                throw argTypeError(
                    `The "maxTotalSockets" argument must be of type number. Received ${receivedRepr(value)}`);
            }
            if (Number.isNaN(value) || value <= 0) {
                const err = new RangeError(
                    `The value of "maxTotalSockets" is out of range. It must be > 0. Received ${value}`);
                err.code = 'ERR_OUT_OF_RANGE';
                throw err;
            }
        }
        this.maxTotalSockets = this.options.maxTotalSockets !== undefined
            ? this.options.maxTotalSockets : Infinity;
        this.requests = {};
        this.sockets = {};
        this.freeSockets = {};
        this.scheduling = this.options.scheduling || 'lifo';
    }

    _pool(map, name) {
        return map[name] ||= [];
    }

    _removeFrom(map, name, socket) {
        const list = map[name];
        if (!list) return;
        const index = list.indexOf(socket);
        if (index !== -1) list.splice(index, 1);
        if (list.length === 0) delete map[name];
    }

    get totalSocketCount() {
        let count = 0;
        for (const list of Object.values(this.sockets)) count += list.length;
        for (const list of Object.values(this.freeSockets)) count += list.length;
        return count;
    }

    // Response finished on a reusable connection: hand the socket to a
    // queued request, park it in freeSockets, or close it.
    releaseSocket(socket, name) {
        this._removeFrom(this.sockets, name, socket);
        const queue = this.requests[name];
        if (queue && queue.length > 0) {
            const req = queue.shift();
            if (queue.length === 0) delete this.requests[name];
            this._dispatch(req, socket, name);
            return;
        }
        if (!this.keepAlive || socket.destroyed) {
            socket.destroy();
            return;
        }
        const free = this._pool(this.freeSockets, name);
        if (free.length >= this.maxFreeSockets) {
            socket.destroy();
            return;
        }
        free.push(socket);
        socket.unref();
        socket.emit('free');
        this.emit('free', socket, socket._agentOptions || {});
    }

    _dispatch(req, socket, name) {
        socket.ref();
        req.reusedSocket = true;
        this._pool(this.sockets, name).push(socket);
        req._adoptSocket(socket);
    }

    removeSocket(socket, name) {
        this._removeFrom(this.sockets, name, socket);
        this._removeFrom(this.freeSockets, name, socket);
        // Freed capacity dispatches a queued request for this name.
        const queue = this.requests[name];
        if (queue && queue.length > 0) {
            const req = queue.shift();
            if (queue.length === 0) delete this.requests[name];
            Promise.resolve().then(() =>
                this.addRequest(req, req._queuedOptions || {}));
        }
    }

    createConnection(options, callback) {
        const socket = net.connect(options);
        if (typeof callback === 'function') {
            socket.once('connect', () => callback(null, socket));
            socket.once('error', (error) => callback(error));
        }
        return socket;
    }
    // The layer ClientRequest goes through; tests stub either this or
    // createConnection.
    createSocket(req, options, cb) {
        let returned;
        try {
            returned = this.createConnection(options, (error, socket) => {
                if (error) cb(error);
                else if (!returned) cb(null, socket);
            });
        } catch (error) {
            cb(error);
            return;
        }
        if (returned) cb(null, returned);
    }
    // The layer requests attach through when constructed with an agent
    // externally (tests drive it directly too).
    addRequest(req, options) {
        const name = this.getName({ ...this.options, ...options });
        req._agent = this;
        req._agentName = name;
        const free = this.freeSockets[name];
        if (free && free.length > 0) {
            const socket = this.scheduling === 'fifo' ? free.shift() : free.pop();
            if (free.length === 0) delete this.freeSockets[name];
            if (!socket.destroyed) {
                this._dispatch(req, socket, name);
                return;
            }
        }
        const busy = (this.sockets[name] || []).length;
        if (busy >= this.maxSockets || this.totalSocketCount >= this.maxTotalSockets) {
            req._queuedOptions = { ...options };
            this._pool(this.requests, name).push(req);
            return;
        }
        this.createSocket(req, { ...this.options, ...options }, (error, socket) => {
            if (error) {
                Promise.resolve().then(() => {
                    req.emit('error', error);
                    req.destroy();
                });
                return;
            }
            if (socket && typeof socket.on === 'function') {
                socket._agentOptions = { ...options };
                this._pool(this.sockets, name).push(socket);
                socket.on('close', () => this.removeSocket(socket, name));
            }
            if (typeof req._adoptSocket === 'function') {
                req._adoptSocket(socket);
            }
        });
    }
    keepSocketAlive(socket) {
        socket.unref();
        return true;
    }
    reuseSocket(socket, req) {
        socket.ref();
        req.reusedSocket = true;
    }
    destroy() {
        for (const map of [this.sockets, this.freeSockets]) {
            for (const list of Object.values(map)) {
                for (const socket of [...list]) socket.destroy();
            }
        }
        this.sockets = {};
        this.freeSockets = {};
        this.requests = {};
    }
    getName(options = {}) {
        let name = `${options.host || 'localhost'}:${options.port || ''}:`;
        if (options.localAddress) name += options.localAddress;
        if (options.family === 4 || options.family === 6) name += `:${options.family}`;
        if (options.socketPath) name += `:${options.socketPath}`;
        return name;
    }
}

export const Agent = callable(AgentImpl);
export const globalAgent = new AgentImpl({ keepAlive: true });

function normalizeRequestArgs(input, options, cb) {
    let opts = {};
    if (typeof input === 'string' || (input && input.href && input.hostname !== undefined)) {
        const isWhatwg = typeof input !== 'string' && typeof input.searchParams === 'object';
        const url = typeof input === 'string' ? new URL(input) : input;
        if (typeof input !== 'string' && !isWhatwg) {
            // Legacy url.parse object: extra properties (method, headers,
            // agent, ...) ride along.
            opts = { ...input };
        }
        opts.hostname = url.hostname;
        opts.port = url.port ? Number(url.port) : 80;
        opts.path = (typeof url.path === 'string' && url.path)
            || `${url.pathname || '/'}${url.search || ''}` || '/';
        if (!opts.auth && (url.username || url.password)) {
            opts.auth = `${decodeURIComponent(url.username || '')}:${decodeURIComponent(url.password || '')}`;
        }
        if (url.protocol && url.protocol !== 'http:') {
            const err = new TypeError(
                `Protocol "${url.protocol}" not supported. Expected "http:"`);
            err.code = 'ERR_INVALID_PROTOCOL';
            throw err;
        }
        if (typeof options === 'function') {
            cb = options;
        } else if (options) {
            opts = { ...opts, ...options };
        }
    } else {
        if (typeof options === 'function') cb = options;
        opts = { ...input };
    }
    return [opts, cb];
}

class ClientRequestImpl extends OutgoingMessageImpl {
    constructor(opts, cb) {
        super();
        validateRequestOptions(opts);
        this.method = String(opts.method || 'GET').toUpperCase();
        this._path = opts.path || '/';
        this.host = opts.hostname || opts.host || 'localhost';
        this.port = Number(opts.port)
            || (opts.agent && Number(opts.agent.defaultPort))
            || (opts._defaultAgent && Number(opts._defaultAgent.defaultPort))
            || 80;
        this.res = null;
        this.aborted = false;
        this.reusedSocket = false;
        this._timeoutOpt = typeof opts.timeout === 'number' ? opts.timeout : null;
        this._joinDuplicateHeaders = Boolean(opts.joinDuplicateHeaders);
        this._headersFromArray = Array.isArray(opts.headers);
        // Node's req emits 'close' when the underlying socket is done, not
        // when the writable side finishes; suppress the stream-level close.
        this._closeEmitted = true;
        if (cb) this.once('response', cb);
        if (opts.headers) {
            if (Array.isArray(opts.headers)) {
                if (opts.headers.length > 0 && Array.isArray(opts.headers[0])) {
                    for (const [name, value] of opts.headers) {
                        this.appendHeader(name, value);
                    }
                } else {
                    for (let i = 0; i + 1 < opts.headers.length; i += 2) {
                        this.appendHeader(opts.headers[i], opts.headers[i + 1]);
                    }
                }
            } else {
                for (const name of Object.keys(opts.headers)) {
                    this.setHeader(name, opts.headers[name]);
                }
            }
        }
        if (opts.auth && !this._headersFromArray && !this.hasHeader('authorization')) {
            this._headers.set('authorization', ['Authorization',
                'Basic ' + Buffer.from(String(opts.auth)).toString('base64')]);
        }
        // A user createConnection (or one from a custom agent) may hand back
        // any duplex stream — the generic-streams pattern — either as a
        // return value or through a (err, socket) callback; otherwise dial
        // the loopback TCP transport.
        const connectOptions = { ...opts, port: this.port, host: this.host };
        // In socket options "path" means an IPC pipe; the request path must
        // not leak through (Node nulls it the same way).
        delete connectOptions.path;
        if (opts.socketPath) connectOptions.path = opts.socketPath;
        let settled = false;
        const settle = (error, socket) => {
            if (settled) return;
            settled = true;
            if (error) {
                Promise.resolve().then(() => {
                    this.emit('error', error);
                    this.destroy();
                });
            } else {
                this._adoptSocket(socket);
            }
        };
        if (typeof opts.createConnection === 'function') {
            let returned;
            try {
                returned = opts.createConnection(connectOptions, settle);
            } catch (error) {
                settle(error);
                return;
            }
            if (returned && !settled) settle(null, returned);
        } else if (opts.agent && typeof opts.agent.addRequest === 'function') {
            opts.agent.addRequest(this, connectOptions);
        } else if (opts.agent === false || opts.createConnection) {
            this._adoptSocket(net.connect({ port: this.port, host: this.host }));
        } else {
            // Node routes through the (keep-alive) global agent by default.
            globalAgent.addRequest(this, connectOptions);
        }
        // GET/HEAD requests without a body are finalized by http.get() or by
        // the caller invoking end(); nothing is sent until then.
    }

    _adoptSocket(socket) {
        this.socket = socket;
        this.connection = socket;
        if (!socket || typeof socket.on !== 'function') {
            // createConnection returned nothing usable; surface the socket
            // event with what it gave us and go no further, matching the
            // construction-only tests.
            Promise.resolve().then(() => this.emit('socket', socket));
            return;
        }
        socket._currentRequest = this;
        this._responseParser = new ResponseParser(this, socket);
        socket._currentParser = this._responseParser;
        // Socket-level handlers are wired once; a pooled socket delegates to
        // whichever request currently owns it.
        if (!socket._httpClientWired) {
            socket._httpClientWired = true;
            if (typeof socket.on === 'function') {
                socket.on('timeout', () => {
                    if (socket._currentRequest) socket._currentRequest.emit('timeout');
                });
                socket.on('error', (error) => {
                    const req = socket._currentRequest;
                    if (req) req._sawError = true;
                    if (req && !req.aborted) req.emit('error', error);
                });
                socket.on('data', (chunk) => {
                    try {
                        if (socket._currentParser) socket._currentParser.feed(chunk);
                    } catch (error) {
                        socket.destroy(error);
                    }
                });
                socket.on('close', () => {
                    const parser = socket._currentParser;
                    const req = socket._currentRequest;
                    if (parser) parser.onClose();
                    if (req) req._emitReqClose();
                });
            }
        }
        if (typeof socket.setTimeout === 'function' && this._timeoutOpt !== null) {
            socket.setTimeout(this._timeoutOpt);
        }
        if (socket.connecting) {
            // Node emits 'socket' at assignment, before the connection
            // completes, so listeners can attach to 'connect' in time.
            Promise.resolve().then(() => this.emit('socket', socket));
            socket.on('connect', () => {
                if (socket._currentRequest !== this) return;
                if (this._pendingTimeout !== undefined
                    && typeof socket.setTimeout === 'function') {
                    socket.setTimeout(this._pendingTimeout, this._pendingTimeoutCb);
                    this._pendingTimeout = undefined;
                    this._pendingTimeoutCb = undefined;
                }
                this._connected = true;
                this.emit('_ready');
            });
        } else {
            // Pre-connected (reused pooled socket, or a generic duplex).
            this._connected = true;
            Promise.resolve().then(() => {
                if (this._pendingTimeout !== undefined
                    && typeof socket.setTimeout === 'function') {
                    socket.setTimeout(this._pendingTimeout, this._pendingTimeoutCb);
                    this._pendingTimeout = undefined;
                    this._pendingTimeoutCb = undefined;
                }
                this.emit('socket', socket);
                this.emit('_ready');
            });
        }
    }

    _emitReqClose() {
        if (this._reqClosed) return;
        this._reqClosed = true;
        this.emit('close');
    }

    // Node validates on every assignment (TOCTOU regression contract).
    get path() {
        return this._path;
    }

    set path(value) {
        if (INVALID_PATH_RE.test(String(value))) {
            const err = new TypeError('Request path contains unescaped characters');
            err.code = 'ERR_UNESCAPED_CHARACTERS';
            throw err;
        }
        this._path = value;
    }

    setTimeout(ms, callback) {
        if (this.socket && this._connected && typeof this.socket.setTimeout === 'function') {
            this.socket.setTimeout(ms, callback);
        } else {
            // Applied once the socket connects (Node's assignment timing).
            this._pendingTimeout = ms;
            this._pendingTimeoutCb = callback;
            if (typeof callback === 'function') this.once('timeout', callback);
        }
        return this;
    }

    end(chunk, encoding, callback) {
        if (!this.headersSent && chunk != null && !this._hasBody
            && !this.hasHeader('content-length') && !this.hasHeader('transfer-encoding')) {
            const length = Buffer.isBuffer(chunk)
                ? chunk.length
                : Buffer.byteLength(String(chunk), typeof encoding === 'string' ? encoding : 'utf8');
            this._headers.set('content-length', ['Content-Length', String(length)]);
        }
        return super.end(chunk, encoding, callback);
    }

    _flushHead() {
        if (this.headersSent) return;
        this.headersSent = true;
        if (!this.hasHeader('host') && !this._headersFromArray) {
            const hostHeader = this.port === 80 ? this.host : `${this.host}:${this.port}`;
            const entries = [...this._headers];
            this._headers.clear();
            this._headers.set('host', ['Host', hostHeader]);
            for (const [key, entry] of entries) this._headers.set(key, entry);
        }
        if (!this.hasHeader('connection')) {
            // Node's default agent advertises keep-alive; our transport still
            // uses one connection per request (the response parser closes it).
            this._headers.set('connection', ['Connection', 'keep-alive']);
        }
        const bodyless = this.method === 'GET' || this.method === 'HEAD'
            || this.method === 'DELETE' || this.method === 'OPTIONS';
        if (!this.hasHeader('content-length') && !this.hasHeader('transfer-encoding')) {
            if (this._hasBody || !bodyless) {
                this._chunked = true;
                this._headers.set('transfer-encoding', ['Transfer-Encoding', 'chunked']);
            }
        } else if (/chunked/i.test(String(this.getHeader('transfer-encoding') || ''))) {
            this._chunked = true;
        }
        this._sendRaw(
            `${this.method} ${this.path} HTTP/1.1\r\n` +
            this._serializeHeaders() + '\r\n');
    }

    _sendRaw(data) {
        if (this._connected) {
            this.socket.write(data);
        } else {
            this.once('_ready', () => this.socket.write(data));
        }
    }

    _writeRaw(data) {
        this._sendRaw(data);
    }

    _write(chunk, encoding, callback) {
        this._hasBody = true;
        super._write(chunk, encoding, callback);
    }

    _afterFinal() {
        // Response handling closes the socket; nothing to do at request end.
    }

    abort() {
        if (this.aborted) return;
        this.aborted = true;
        this.emit('abort');
        this.destroy();
    }

    destroy(err) {
        if (this.destroyed) return this;
        if (this._agent && this._agentName) {
            const queue = this._agent.requests[this._agentName];
            if (queue) {
                const index = queue.indexOf(this);
                if (index !== -1) {
                    queue.splice(index, 1);
                    if (queue.length === 0) delete this._agent.requests[this._agentName];
                }
            }
        }
        // A socket already released back to the pool is no longer this
        // request's to destroy.
        const owned = this.socket && typeof this.socket.destroy === 'function'
            && this.socket._currentRequest === this;
        if (owned && !this.socket.destroyed) this.socket.destroy();
        const result = super.destroy(err);
        if (!owned) Promise.resolve().then(() => this._emitReqClose());
        return result;
    }

    setNoDelay() {}
    setSocketKeepAlive() {}
}

class ResponseParser {
    constructor(request, socket) {
        this.request = request;
        this.socket = socket;
        this.buf = EMPTY;
        this.res = null;
        this.remaining = null;      // content-length countdown; null = until close
        this.chunked = false;
        this.chunkRemaining = null;
        this.done = false;
    }

    feed(chunk) {
        if (this.done) return;
        if (!Buffer.isBuffer(chunk)) chunk = Buffer.from(chunk);
        this.buf = this.buf.length === 0 ? chunk : Buffer.concat([this.buf, chunk]);
        this.process();
    }

    process() {
        if (this.res === null) {
            const headEnd = this.buf.indexOf('\r\n\r\n');
            if (headEnd === -1) return;
            const head = this.buf.subarray(0, headEnd).toString('latin1');
            this.buf = this.buf.subarray(headEnd + 4);
            const lines = head.split('\r\n');
            const match = /^HTTP\/(\d)\.(\d) (\d{3}) ?(.*)$/.exec(lines[0]);
            if (!match) {
                this.socket.destroy(new Error('Parse Error: invalid status line'));
                return;
            }
            const res = new IncomingMessageImpl(this.socket);
            res.httpVersionMajor = Number(match[1]);
            res.httpVersionMinor = Number(match[2]);
            res.httpVersion = `${match[1]}.${match[2]}`;
            res.statusCode = Number(match[3]);
            res.statusMessage = match[4];
            for (let i = 1; i < lines.length; i++) {
                const sep = lines[i].indexOf(':');
                if (sep === -1) continue;
                addIncomingHeader(
                    res.headers, res.rawHeaders,
                    lines[i].slice(0, sep).trim(), lines[i].slice(sep + 1).trim(),
                    Boolean(this.request._joinDuplicateHeaders));
            }
            this.res = res;
            this.request.res = res;
            // HTTP Upgrade / CONNECT: hand the raw socket to the user. A 101
            // (or an Upgrade + Connection: upgrade response), and a 2xx to a
            // CONNECT request, detach the socket from HTTP parsing and emit
            // 'upgrade'/'connect' with (res, socket, head). With no listener,
            // Node closes the socket.
            const isConnect = this.request.method === 'CONNECT';
            const upgradeHeader = res.headers['upgrade'];
            const connUpgrade = String(res.headers['connection'] || '')
                .toLowerCase().split(',').some((t) => t.trim() === 'upgrade');
            const isUpgrade = res.statusCode === 101
                || (upgradeHeader !== undefined && connUpgrade);
            if (isUpgrade || (isConnect && res.statusCode >= 200 && res.statusCode < 300)) {
                this.done = true;
                res.upgrade = true;
                const head = this.buf;
                this.buf = EMPTY;
                if (this.socket) {
                    this.socket._currentParser = null;
                    this.socket._currentRequest = null;
                    // The socket is now the user's; don't pool or auto-manage it.
                    this.socket._httpUpgraded = true;
                }
                this.request._upgraded = true;
                const eventName = isConnect ? 'connect' : 'upgrade';
                const hadListener =
                    this.request.emit(eventName, res, this.socket, head);
                if (!hadListener && this.socket) this.socket.destroy();
                // The request itself is finished once upgraded; Node emits
                // its 'close' after the upgrade is handed off.
                Promise.resolve().then(() => this.request._emitReqClose());
                return;
            }
            // A 1xx (other than 101, handled above) is an interim
            // informational response: emit 'information' and keep parsing
            // for the real response that follows.
            if (res.statusCode >= 100 && res.statusCode < 200) {
                this.res = null;
                this.request.res = null;
                this.request.emit('information', {
                    httpVersion: res.httpVersion,
                    httpVersionMajor: res.httpVersionMajor,
                    httpVersionMinor: res.httpVersionMinor,
                    statusCode: res.statusCode,
                    statusMessage: res.statusMessage,
                    headers: res.headers,
                    rawHeaders: res.rawHeaders,
                });
                if (this.buf.length > 0) this.process();
                return;
            }
            const te = String(res.headers['transfer-encoding'] || '').toLowerCase();
            const contentLength = res.headers['content-length'];
            const noBody = this.request.method === 'HEAD'
                || res.statusCode === 204 || res.statusCode === 304;
            if (noBody) {
                this.remaining = 0;
            } else if (te.includes('chunked')) {
                this.chunked = true;
                this.chunkRemaining = null;
            } else if (contentLength !== undefined) {
                this.remaining = Number(contentLength);
                if (!Number.isFinite(this.remaining) || this.remaining < 0) {
                    this.remaining = 0;
                }
            } else {
                this.remaining = null; // read until close
            }
            this.request.emit('response', res);
            if (this.remaining === 0) { this.finish(); return; }
        }
        if (this.done) return;
        if (this.chunked) {
            this.processChunked();
            return;
        }
        if (this.remaining === null) {
            if (this.buf.length > 0) {
                this.res.push(this.buf);
                this.buf = EMPTY;
            }
            return;
        }
        if (this.buf.length > 0 && this.remaining > 0) {
            const take = Math.min(this.remaining, this.buf.length);
            this.res.push(this.buf.subarray(0, take));
            this.buf = this.buf.subarray(take);
            this.remaining -= take;
        }
        if (this.remaining === 0) this.finish();
    }

    processChunked() {
        for (;;) {
            if (this.chunkRemaining === null) {
                const lineEnd = this.buf.indexOf('\r\n');
                if (lineEnd === -1) return;
                const sizeLine = this.buf.subarray(0, lineEnd).toString('latin1');
                this.buf = this.buf.subarray(lineEnd + 2);
                if (!/^[0-9a-fA-F]+(;.*)?$/.test(sizeLine)) {
                    this.socket.destroy(new Error('Parse Error: Invalid character in chunk size'));
                    return;
                }
                const size = parseInt(sizeLine, 16);
                if (size === 0) { this.chunkRemaining = 0; }
                else this.chunkRemaining = size;
            }
            if (this.chunkRemaining === -1) {
                if (this.buf.length < 2) return;
                this.buf = this.buf.subarray(2);
                this.chunkRemaining = null;
                continue;
            }
            if (this.chunkRemaining === 0) {
                for (;;) {
                    const lineEnd = this.buf.indexOf('\r\n');
                    if (lineEnd === -1) return;
                    const line = this.buf.subarray(0, lineEnd).toString('latin1');
                    this.buf = this.buf.subarray(lineEnd + 2);
                    if (!line) { this.finish(); return; }
                    const sep = line.indexOf(':');
                    if (sep !== -1 && this.res) {
                        const name = line.slice(0, sep).trim();
                        const value = line.slice(sep + 1).trim();
                        this.res.rawTrailers.push(name, value);
                        const key = name.toLowerCase();
                        this.res.trailers[key] = Object.hasOwn(this.res.trailers, key)
                            ? this.res.trailers[key] + ', ' + value : value;
                    }
                }
            }
            if (this.buf.length === 0) return;
            const take = Math.min(this.chunkRemaining, this.buf.length);
            this.res.push(this.buf.subarray(0, take));
            this.buf = this.buf.subarray(take);
            this.chunkRemaining -= take;
            if (this.chunkRemaining === 0) {
                if (this.buf.length < 2) { this.chunkRemaining = -1; return; }
                this.buf = this.buf.subarray(2);
                this.chunkRemaining = null;
            }
        }
    }

    finish() {
        if (this.done) return;
        this.done = true;
        this.res.complete = true;
        // Node marks the message destroyed once fully consumed, before its
        // 'close' event.
        this.res.once('end', () => { this.res.destroyed = true; });
        this.request.destroyed = true;
        const request = this.request;
        const socket = this.socket;
        const connectionHeader = String(this.res.headers.connection || '');
        socket._currentParser = null;
        this.res.once('end', () => {
            request._emitReqClose();
            const agent = request._agent;
            const reusable = agent && agent.keepAlive && !socket.destroyed
                && !/close/i.test(connectionHeader);
            if (reusable) {
                socket._currentRequest = null;
                agent.releaseSocket(socket, request._agentName);
            } else if (!socket.destroyed) {
                socket.destroy();
            }
        });
        this.res.push(null);
    }

    onClose() {
        if (this.done) return;
        if (!this.res) {
            // Connection ended before any response: Node's classic
            // "socket hang up", suppressed for an explicit abort().
            if (!this.request.aborted && !this.request._sawError
                && !this.request._hangupEmitted) {
                this.request._hangupEmitted = true;
                const err = new Error('socket hang up');
                err.code = 'ECONNRESET';
                this.request.emit('error', err);
            }
            return;
        }
        if (this.res && this.remaining === null && !this.chunked) {
            // Read-until-close body: EOF terminates it cleanly.
            this.finish();
        } else if (this.res) {
            this.res.aborted = true;
            this.res.emit('aborted');
            if (this.res.listenerCount('error') > 0) {
                const err = new Error('aborted');
                err.code = 'ECONNRESET';
                this.res.emit('error', err);
            }
            this.res.push(null);
        }
    }
}

export function request(input, options, cb) {
    if (!netEnabled) {
        throw new Error(
            'http.request is not supported in this runtime: use fetch() (HTTP/1) ' +
            'or node:http2 instead');
    }
    const [opts, callback] = normalizeRequestArgs(input, options, cb);
    return new ClientRequestImpl(opts, callback);
}

export function get(input, options, cb) {
    const req = request(input, options, cb);
    req.end();
    return req;
}

export function setMaxIdleHTTPParsers() {}

export const IncomingMessage = callable(IncomingMessageImpl);
export const OutgoingMessage = callable(OutgoingMessageImpl);
export const ServerResponse = callable(ServerResponseImpl);
export const Server = callable(ServerImpl);
export const ClientRequest = callable(ClientRequestImpl);

export default {
    METHODS,
    STATUS_CODES,
    maxHeaderSize,
    validateHeaderName,
    validateHeaderValue,
    Agent,
    globalAgent,
    ClientRequest,
    IncomingMessage,
    OutgoingMessage,
    ServerResponse,
    Server,
    createServer,
    request,
    get,
    setMaxIdleHTTPParsers,
};
