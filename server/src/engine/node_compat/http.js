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

export class IncomingMessage extends Readable {
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
        if (this.socket && !this.socket.destroyed) this.socket.destroy(err);
        return super.destroy(err);
    }
}

// ── outgoing message (shared by ServerResponse / ClientRequest) ─────────

export class OutgoingMessage extends Writable {
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

    setTimeout(ms, callback) {
        if (this.socket) this.socket.setTimeout(ms, callback);
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
            this.socket.write(data);
        }
    }

    _write(chunk, encoding, callback) {
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

export class ServerResponse extends OutgoingMessage {
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
        if (!this._keepAlive && socket && !socket.destroyed) {
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
    }

    feed(chunk) {
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
        const req = new IncomingMessage(this.socket);
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
        const res = new ServerResponse(req);
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
            req.push(null);
        }
    }
}

// ── server ──────────────────────────────────────────────────────────────

export class Server extends net.Server {
    constructor(options, requestListener) {
        if (typeof options === 'function') {
            requestListener = options;
            options = {};
        }
        super(options || {});
        this.timeout = 0;
        this.keepAliveTimeout = 5000;
        this.headersTimeout = 60000;
        this.requestTimeout = 300000;
        if (typeof requestListener === 'function') {
            this.on('request', requestListener);
        }
        this.on('connection', (socket) => this._setupConnection(socket));
    }

    _setupConnection(socket) {
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
    }

    setTimeout(ms, callback) {
        this.timeout = ms;
        if (callback) this.on('timeout', callback);
        return this;
    }

    closeAllConnections() {
        for (const socket of [...this._connections]) socket.destroy();
    }

    closeIdleConnections() {
        this.closeAllConnections();
    }
}

export function createServer(options, requestListener) {
    if (!netEnabled) {
        throw new Error('http.createServer is not supported in this runtime');
    }
    return new Server(options, requestListener);
}

// ── client ──────────────────────────────────────────────────────────────

export class Agent extends EventEmitter {
    constructor(options) {
        super();
        this.options = options || {};
        this.keepAlive = Boolean(this.options.keepAlive);
        this.maxSockets = this.options.maxSockets || Infinity;
        this.requests = {};
        this.sockets = {};
        this.freeSockets = {};
    }
    destroy() {}
    getName(options) {
        return `${options.host || 'localhost'}:${options.port || 80}:`;
    }
}

export const globalAgent = new Agent({ keepAlive: true });

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

export class ClientRequest extends OutgoingMessage {
    constructor(opts, cb) {
        super();
        this.method = String(opts.method || 'GET').toUpperCase();
        this.path = opts.path || '/';
        this.host = opts.hostname || opts.host || 'localhost';
        this.port = Number(opts.port) || 80;
        this.res = null;
        this.aborted = false;
        this.reusedSocket = false;
        if (cb) this.once('response', cb);
        if (opts.headers) {
            for (const name of Object.keys(opts.headers)) {
                this.setHeader(name, opts.headers[name]);
            }
        }
        const socket = net.connect({ port: this.port, host: this.host });
        this.socket = socket;
        this.connection = socket;
        socket.on('connect', () => {
            this.emit('socket', socket);
            this._connected = true;
            this.emit('_ready');
        });
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
            this.emit('close');
        });
        // GET/HEAD requests without a body are finalized by http.get() or by
        // the caller invoking end(); nothing is sent until then.
    }

    _flushHead() {
        if (this.headersSent) return;
        this.headersSent = true;
        if (!this.hasHeader('host')) {
            const hostHeader = this.port === 80 ? this.host : `${this.host}:${this.port}`;
            this._headers.set('host', ['Host', hostHeader]);
        }
        if (!this.hasHeader('connection')) {
            this._headers.set('connection', ['Connection', 'close']);
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
        this.aborted = true;
        this.destroy();
    }

    destroy(err) {
        if (this.socket && !this.socket.destroyed) this.socket.destroy();
        return super.destroy(err);
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
            const res = new IncomingMessage(this.socket);
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
        this.res.push(null);
        // One request per connection (Connection: close by default).
        const socket = this.socket;
        Promise.resolve().then(() => {
            if (!socket.destroyed) socket.destroy();
        });
    }

    onClose() {
        if (this.done) return;
        if (this.res && this.remaining === null && !this.chunked) {
            // Read-until-close body: EOF terminates it cleanly.
            this.finish();
        } else if (this.res) {
            this.res.aborted = true;
            this.res.emit('aborted');
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
    return new ClientRequest(opts, callback);
}

export function get(input, options, cb) {
    const req = request(input, options, cb);
    req.end();
    return req;
}

export function setMaxIdleHTTPParsers() {}

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
