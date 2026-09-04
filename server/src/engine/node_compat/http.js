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
    if (/[\r\n]/.test(String(value))) {
        const err = new TypeError(
            `Invalid character in header content ["${name}"]`);
        err.code = 'ERR_INVALID_CHAR';
        throw err;
    }
}

// ── header collection helpers ───────────────────────────────────────────

function addIncomingHeader(headers, rawHeaders, name, value) {
    rawHeaders.push(name, value);
    const key = name.toLowerCase();
    const existing = headers[key];
    if (existing === undefined) {
        headers[key] = value;
    } else if (key === 'set-cookie') {
        existing.push(value);
    } else if (key === 'cookie') {
        headers[key] = existing + '; ' + value;
    } else {
        headers[key] = existing + ', ' + value;
    }
    if (key === 'set-cookie' && !Array.isArray(headers[key])) {
        headers[key] = [value];
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

    addTrailers(_trailers) {}

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
            this._writeRaw(Buffer.from('0\r\n\r\n'));
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
        if (headers) {
            if (Array.isArray(headers)) {
                for (let i = 0; i + 1 < headers.length; i += 2) {
                    this.setHeader(headers[i], headers[i + 1]);
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
        if (!this.headersSent && !this._wroteBody && !this._suppressBody
            && !this.hasHeader('content-length') && !this.hasHeader('transfer-encoding')) {
            const length = chunk == null ? 0 : (Buffer.isBuffer(chunk)
                ? chunk.length
                : Buffer.byteLength(String(chunk), typeof encoding === 'string' ? encoding : 'utf8'));
            this.setHeaderInternal('Content-Length', String(length));
        }
        return super.end(chunk, encoding, callback);
    }

    writeContinue() {
        this._writeRaw('HTTP/1.1 100 Continue\r\n\r\n');
    }

    writeProcessing() {
        this._writeRaw('HTTP/1.1 102 Processing\r\n\r\n');
    }

    addTrailers(_trailers) {}

    _flushHead() {
        if (this.headersSent) return;
        this.headersSent = true;
        const status = this.statusCode;
        const message = this.statusMessage !== undefined
            ? this.statusMessage
            : (STATUS_CODES[status] || 'unknown');
        const noBody = this._suppressBody
            || status === 204 || status === 304 || (status >= 100 && status < 200);
        if (!this.hasHeader('content-length') && !this.hasHeader('transfer-encoding')
            && !noBody) {
            this._chunked = true;
            this.setHeaderInternal('Transfer-Encoding', 'chunked');
        }
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
        if (!Buffer.isBuffer(chunk)) chunk = Buffer.from(chunk);
        this.buf = this.buf.length === 0 ? chunk : Buffer.concat([this.buf, chunk]);
        this.process();
    }

    process() {
        for (;;) {
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
                lines[i].slice(0, sep).trim(), lines[i].slice(sep + 1).trim());
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
        const res = new ServerResponseImpl(req);
        this.activeReq = req;
        this.activeRes = res;
        res.on('finish', () => {
            if (this.activeRes === res) {
                this.activeReq = null;
                this.activeRes = null;
            }
        });
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
        return true;
    }

    processChunked() {
        for (;;) {
            if (this.chunkRemaining === null) {
                const lineEnd = this.buf.indexOf('\r\n');
                if (lineEnd === -1) return false;
                const size = parseInt(this.buf.subarray(0, lineEnd).toString('latin1'), 16);
                this.buf = this.buf.subarray(lineEnd + 2);
                if (!Number.isFinite(size)) {
                    this.socket.destroy(new Error('invalid chunk size'));
                    return false;
                }
                if (size === 0) {
                    // Trailer section ends with a blank line, which may be
                    // the very next CRLF.
                    const trailerEnd = this.buf.indexOf('\r\n');
                    if (trailerEnd === -1) { this.chunkRemaining = 0; return false; }
                    this.buf = this.buf.subarray(trailerEnd + 2);
                    this.endBody();
                    return true;
                }
                this.chunkRemaining = size;
            }
            if (this.chunkRemaining === 0) {
                // Waiting for the final CRLF after a zero-size line.
                const trailerEnd = this.buf.indexOf('\r\n');
                if (trailerEnd === -1) return false;
                this.buf = this.buf.subarray(trailerEnd + 2);
                this.endBody();
                return true;
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
        this.createSocket(req, { ...this.options, ...options }, (error, socket) => {
            if (error) {
                Promise.resolve().then(() => req.emit('error', error));
            } else if (typeof req._adoptSocket === 'function') {
                req._adoptSocket(socket);
            }
        });
    }
    keepSocketAlive() { return false; }
    reuseSocket() {}
    destroy() {}
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
        const url = typeof input === 'string' ? new URL(input) : input;
        opts.hostname = url.hostname;
        opts.port = url.port ? Number(url.port) : 80;
        opts.path = `${url.pathname}${url.search || ''}` || '/';
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
        // Node's req emits 'close' when the underlying socket is done, not
        // when the writable side finishes; suppress the stream-level close.
        this._closeEmitted = true;
        if (cb) this.once('response', cb);
        if (opts.headers) {
            for (const name of Object.keys(opts.headers)) {
                this.setHeader(name, opts.headers[name]);
            }
        }
        // A user createConnection (or one from a custom agent) may hand back
        // any duplex stream — the generic-streams pattern — either as a
        // return value or through a (err, socket) callback; otherwise dial
        // the loopback TCP transport.
        const connectOptions = { ...opts, port: this.port, host: this.host };
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
        } else if (opts.agent && typeof opts.agent.createSocket === 'function') {
            opts.agent.createSocket(this, connectOptions, settle);
        } else if (opts.agent && typeof opts.agent.createConnection === 'function') {
            settle(null, opts.agent.createConnection(connectOptions));
        } else {
            this._adoptSocket(net.connect({ port: this.port, host: this.host }));
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
        if (typeof socket.setTimeout === 'function') {
            if (this._timeoutOpt !== null) socket.setTimeout(this._timeoutOpt);
            socket.on('timeout', () => this.emit('timeout'));
        }
        if (socket.connecting) {
            // Node emits 'socket' at assignment, before the connection
            // completes, so listeners can attach to 'connect' in time.
            Promise.resolve().then(() => this.emit('socket', socket));
            socket.on('connect', () => {
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
            // Pre-connected duplex (generic stream): usable immediately.
            this._connected = true;
            Promise.resolve().then(() => {
                this.emit('socket', socket);
                this.emit('_ready');
            });
        }
        socket.on('error', (error) => {
            if (!this.aborted) this.emit('error', error);
        });
        this._responseParser = new ResponseParser(this, socket);
        socket.on('data', (chunk) => {
            try {
                this._responseParser.feed(chunk);
            } catch (error) {
                socket.destroy(error);
            }
        });
        socket.on('close', () => {
            this._responseParser.onClose();
            this._emitReqClose();
        });
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
        if (!this.hasHeader('host')) {
            const hostHeader = this.port === 80 ? this.host : `${this.host}:${this.port}`;
            this._headers.set('host', ['Host', hostHeader]);
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
        const hadSocket = this.socket && typeof this.socket.destroy === 'function';
        if (hadSocket && !this.socket.destroyed) this.socket.destroy();
        const result = super.destroy(err);
        if (!hadSocket) Promise.resolve().then(() => this._emitReqClose());
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
                    lines[i].slice(0, sep).trim(), lines[i].slice(sep + 1).trim());
            }
            this.res = res;
            this.request.res = res;
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
                const size = parseInt(this.buf.subarray(0, lineEnd).toString('latin1'), 16);
                this.buf = this.buf.subarray(lineEnd + 2);
                if (!Number.isFinite(size)) {
                    this.socket.destroy(new Error('Parse Error: invalid chunk size'));
                    return;
                }
                if (size === 0) {
                    const trailerEnd = this.buf.indexOf('\r\n');
                    if (trailerEnd === -1) { this.chunkRemaining = 0; return; }
                    this.buf = this.buf.subarray(trailerEnd + 2);
                    this.finish();
                    return;
                }
                this.chunkRemaining = size;
            }
            if (this.chunkRemaining === 0) {
                const trailerEnd = this.buf.indexOf('\r\n');
                if (trailerEnd === -1) return;
                this.buf = this.buf.subarray(trailerEnd + 2);
                this.finish();
                return;
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
        this.res.push(null);
        // One request per connection (Connection: close by default).
        const socket = this.socket;
        Promise.resolve().then(() => {
            if (!socket.destroyed) socket.destroy();
        });
    }

    onClose() {
        if (this.done) return;
        if (!this.res) {
            // Connection ended before any response: Node's classic
            // "socket hang up", suppressed for an explicit abort().
            if (!this.request.aborted && !this.request._hangupEmitted) {
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
