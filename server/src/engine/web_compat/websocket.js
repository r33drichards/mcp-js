// WHATWG WebSocket API over the policy-gated websocket ops
// (op_ws_connect / op_ws_send / op_ws_recv / op_ws_close / op_ws_drop).
//
// Only injected when a `websocket` policy chain is configured, mirroring
// fetch. Handshake headers are not settable or readable from JS (matching
// the browser API), which is what lets server-side header injection keep
// credentials out of the isolate.
//
// Injected on fresh runtimes and baked into heap snapshots (like the fetch
// wrapper); deno_core rebinds the captured op references on snapshot restore.
(function () {
    'use strict';

    var core = Deno.core;
    var opConnect = core.ops.op_ws_connect;
    var opSend = core.ops.op_ws_send;
    var opRecv = core.ops.op_ws_recv;
    var opClose = core.ops.op_ws_close;
    var opDrop = core.ops.op_ws_drop;

    var CONNECTING = 0, OPEN = 1, CLOSING = 2, CLOSED = 3;

    // Live sockets for this execution. A run's event loop stays alive while
    // any socket is open (matching Deno/Node semantics: close your sockets to
    // let the turn finish). __mcpV8WebSocketCloseAll force-drops everything —
    // used by test harnesses; calling it only affects your own sockets.
    var liveSockets = new Set();
    Object.defineProperty(globalThis, '__mcpV8WebSocketCloseAll', {
        value: function () {
            for (var ws of Array.from(liveSockets)) {
                ws._forceDrop();
            }
        },
        writable: false, enumerable: false, configurable: true,
    });

    // ── CloseEvent (not provided by the events layer) ───────────────────
    if (typeof globalThis.CloseEvent !== 'function') {
        class CloseEvent extends Event {
            constructor(type, eventInitDict) {
                if (arguments.length < 1) {
                    throw new TypeError(
                        "Failed to construct 'CloseEvent': 1 argument required, but only 0 present.");
                }
                super(type, eventInitDict);
                var init = eventInitDict || {};
                this._wasClean = !!init.wasClean;
                this._code = init.code !== undefined ? Number(init.code) & 0xffff : 0;
                this._reason = init.reason !== undefined ? String(init.reason) : '';
            }
            get wasClean() { return this._wasClean; }
            get code() { return this._code; }
            get reason() { return this._reason; }
        }
        Object.defineProperty(CloseEvent.prototype, Symbol.toStringTag, {
            value: 'CloseEvent', configurable: true,
        });
        globalThis.CloseEvent = CloseEvent;
    }

    // ── Helpers ─────────────────────────────────────────────────────────

    function b64FromBytes(bytes) {
        var bin = '';
        var CHUNK = 0x8000;
        for (var i = 0; i < bytes.length; i += CHUNK) {
            bin += String.fromCharCode.apply(null, bytes.subarray(i, i + CHUNK));
        }
        return btoa(bin);
    }

    function bytesFromB64(b64) {
        var bin = atob(b64 || '');
        var out = new Uint8Array(bin.length);
        for (var i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i) & 0xff;
        return out;
    }

    // RFC 6455 requires each subprotocol to be an RFC 2616 token.
    var TOKEN_RE = /^[!#$%&'*+\-.0-9A-Z^_`a-z|~]+$/;

    function parseAndValidateUrl(url) {
        // Per spec, the URL is parsed against the API base URL; in this
        // runtime that exists only when a harness defines `location`.
        var base;
        try {
            if (typeof location !== 'undefined' && location && location.href) {
                base = String(location.href);
            }
        } catch (_e) { /* no usable base */ }
        var parsed;
        try {
            parsed = base === undefined
                ? new URL(String(url))
                : new URL(String(url), base);
        } catch (_e) {
            throw new DOMException(
                "Failed to construct 'WebSocket': The URL '" + url + "' is invalid.",
                'SyntaxError');
        }
        // Modern spec: http(s) is accepted and mapped onto ws(s).
        var scheme = parsed.protocol.replace(/:$/, '');
        if (scheme === 'http') parsed.protocol = 'ws:';
        else if (scheme === 'https') parsed.protocol = 'wss:';
        else if (scheme !== 'ws' && scheme !== 'wss') {
            throw new DOMException(
                "Failed to construct 'WebSocket': The URL's scheme must be either 'ws', 'wss', 'http', or 'https'. '"
                + scheme + "' is not allowed.",
                'SyntaxError');
        }
        if (parsed.hash !== '' || String(parsed.href).endsWith('#')) {
            throw new DOMException(
                "Failed to construct 'WebSocket': The URL contains a fragment.",
                'SyntaxError');
        }
        return parsed;
    }

    function normalizeProtocols(protocols) {
        if (protocols === undefined) return [];
        var list;
        if (typeof protocols === 'string') list = [protocols];
        else if (Array.isArray(protocols)) list = protocols.map(String);
        else list = [String(protocols)];
        var seen = new Set();
        for (var p of list) {
            if (!TOKEN_RE.test(p)) {
                throw new DOMException(
                    "Failed to construct 'WebSocket': The subprotocol '" + p + "' is invalid.",
                    'SyntaxError');
            }
            var key = p.toLowerCase();
            if (seen.has(key)) {
                throw new DOMException(
                    "Failed to construct 'WebSocket': The subprotocol '" + p + "' is duplicated.",
                    'SyntaxError');
            }
            seen.add(key);
        }
        return list;
    }

    function utf8Length(s) {
        var n = 0;
        for (var i = 0; i < s.length; i++) {
            var c = s.codePointAt(i);
            if (c > 0xffff) i++;
            n += c <= 0x7f ? 1 : c <= 0x7ff ? 2 : c <= 0xffff ? 3 : 4;
        }
        return n;
    }

    // ── WebSocket ───────────────────────────────────────────────────────

    class WebSocket extends EventTarget {
        constructor(url, protocols) {
            super();
            if (arguments.length < 1) {
                throw new TypeError(
                    "Failed to construct 'WebSocket': 1 argument required, but only 0 present.");
            }
            var parsed = parseAndValidateUrl(url);
            var protocolList = normalizeProtocols(protocols);

            this._url = parsed.href;
            this._readyState = CONNECTING;
            this._protocol = '';
            this._extensions = '';
            this._binaryType = 'blob';
            this._bufferedAmount = 0;
            this._rid = null;
            this._abortedDuringConnect = false;
            this._handlers = { open: null, message: null, error: null, close: null };

            liveSockets.add(this);
            this._connect(parsed.href, protocolList);
        }

        get url() { return this._url; }
        get readyState() { return this._readyState; }
        get protocol() { return this._protocol; }
        get extensions() { return this._extensions; }
        get bufferedAmount() { return this._bufferedAmount; }

        get binaryType() { return this._binaryType; }
        set binaryType(value) {
            // WebIDL enum attribute: invalid assignments are ignored.
            if (value === 'blob' || value === 'arraybuffer') this._binaryType = value;
        }

        get CONNECTING() { return CONNECTING; }
        get OPEN() { return OPEN; }
        get CLOSING() { return CLOSING; }
        get CLOSED() { return CLOSED; }

        send(data) {
            if (this._readyState === CONNECTING) {
                throw new DOMException(
                    "Failed to execute 'send' on 'WebSocket': Still in CONNECTING state.",
                    'InvalidStateError');
            }
            if (this._readyState !== OPEN) return; // discarded per spec
            var self = this;
            function dispatchFrame(kind, payload, size) {
                self._bufferedAmount += size;
                opSend(self._rid, kind, payload).then(
                    function () { self._bufferedAmount -= size; },
                    function (_e) {
                        self._bufferedAmount -= size;
                        // A send on a socket the peer tore down surfaces as
                        // the close handshake from the recv loop.
                    });
            }

            if (typeof Blob !== 'undefined' && data instanceof Blob) {
                // Per spec a Blob send is async: bufferedAmount grows by
                // blob.size immediately, the bytes are read via the public
                // arrayBuffer() (works for any Blob implementation).
                var blobSize = data.size;
                this._bufferedAmount += blobSize;
                data.arrayBuffer().then(function (buf) {
                    return opSend(self._rid, 'binary', b64FromBytes(new Uint8Array(buf)));
                }).then(
                    function () { self._bufferedAmount -= blobSize; },
                    function (_e) { self._bufferedAmount -= blobSize; });
                return;
            }
            if (typeof data === 'string') {
                dispatchFrame('text', data, utf8Length(data));
            } else if (data instanceof ArrayBuffer) {
                var bytes = new Uint8Array(data);
                dispatchFrame('binary', b64FromBytes(bytes), bytes.length);
            } else if (ArrayBuffer.isView(data)) {
                var view = new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
                dispatchFrame('binary', b64FromBytes(view), view.length);
            } else {
                var text = String(data);
                dispatchFrame('text', text, utf8Length(text));
            }
        }

        close(code, reason) {
            if (code !== undefined) {
                code = Number(code) >>> 0;
                if (code !== 1000 && !(code >= 3000 && code <= 4999)) {
                    throw new DOMException(
                        "Failed to execute 'close' on 'WebSocket': The code must be either 1000, or between 3000 and 4999. "
                        + code + ' is neither.',
                        'InvalidAccessError');
                }
            }
            if (reason !== undefined) {
                reason = String(reason);
                if (utf8Length(reason) > 123) {
                    throw new DOMException(
                        "Failed to execute 'close' on 'WebSocket': The message must not be greater than 123 bytes.",
                        'SyntaxError');
                }
            }
            if (this._readyState === CLOSING || this._readyState === CLOSED) return;
            if (this._readyState === CONNECTING) {
                // Fail the connection once the handshake settles.
                this._readyState = CLOSING;
                this._abortedDuringConnect = true;
                return;
            }
            this._readyState = CLOSING;
            opClose(this._rid, code === undefined ? 0 : code, reason || '').then(
                function () {}, function (_e) {});
        }

        // Internal: drop the native connection without a close handshake.
        _forceDrop() {
            if (this._rid !== null) {
                try { opDrop(this._rid); } catch (_e) {}
            }
            this._readyState = CLOSED;
            liveSockets.delete(this);
        }

        async _connect(href, protocolList) {
            var result;
            try {
                var raw = await opConnect(href, JSON.stringify(protocolList));
                result = JSON.parse(raw);
            } catch (e) {
                this._failConnection(e && e.message ? String(e.message) : '');
                return;
            }
            this._rid = result.rid;
            if (this._readyState === CLOSED) {
                // Force-dropped while the handshake was in flight: release
                // the native connection silently, no events.
                try { opDrop(this._rid); } catch (_e) {}
                return;
            }
            if (this._abortedDuringConnect) {
                // close() was called while CONNECTING: fail the connection.
                opClose(this._rid, 0, '').then(function () {}, function (_e) {});
                this._settleClosed(1006, '', false);
                return;
            }
            this._readyState = OPEN;
            this._protocol = result.protocol || '';
            this._dispatch(new Event('open'));
            this._recvLoop();
        }

        _failConnection(message) {
            this._readyState = CLOSED;
            liveSockets.delete(this);
            this._dispatch(new ErrorEvent('error', { message: message || '' }));
            this._dispatch(new CloseEvent('close', { wasClean: false, code: 1006, reason: '' }));
        }

        _settleClosed(code, reason, wasClean) {
            if (this._rid !== null) {
                try { opDrop(this._rid); } catch (_e) {}
            }
            this._readyState = CLOSED;
            liveSockets.delete(this);
            this._dispatch(new CloseEvent('close', {
                wasClean: !!wasClean, code: code, reason: reason || '',
            }));
        }

        async _recvLoop() {
            while (this._readyState === OPEN || this._readyState === CLOSING) {
                var event;
                try {
                    event = JSON.parse(await opRecv(this._rid));
                } catch (_e) {
                    // The connection was force-dropped (e.g. __mcpV8WebSocketCloseAll).
                    if (this._readyState !== CLOSED) this._settleClosed(1006, '', false);
                    return;
                }
                if (event.kind === 'text' || event.kind === 'binary') {
                    if (this._readyState !== OPEN) continue; // discard after close()
                    var data;
                    if (event.kind === 'text') {
                        data = event.data;
                    } else {
                        var bytes = bytesFromB64(event.data);
                        data = this._binaryType === 'arraybuffer'
                            ? bytes.buffer
                            : new Blob([bytes]);
                    }
                    this._dispatch(new MessageEvent('message', {
                        data: data,
                        origin: this._url.replace(/^(wss?:\/\/[^/]*).*$/, '$1'),
                    }));
                } else if (event.kind === 'close') {
                    // Already force-dropped (op cancelled): exit silently so
                    // no late close event fires after harness/user teardown.
                    if (this._readyState === CLOSED) return;
                    this._settleClosed(event.code, event.reason, event.wasClean);
                    return;
                } else { // "error"
                    this._dispatch(new ErrorEvent('error', { message: event.message || '' }));
                    this._settleClosed(1006, '', false);
                    return;
                }
            }
        }

        _dispatch(event) {
            var handler = this._handlers[event.type];
            if (typeof handler === 'function') {
                var self = this;
                var wrapped = function (e) { handler.call(self, e); };
                this.addEventListener(event.type, wrapped, { once: true });
                try {
                    this.dispatchEvent(event);
                } finally {
                    this.removeEventListener(event.type, wrapped);
                }
            } else {
                this.dispatchEvent(event);
            }
        }
    }

    // Handler (onopen/...) accessors, dispatched after listeners added
    // before the assignment but interleaved via _dispatch above.
    for (var type of ['open', 'message', 'error', 'close']) {
        (function (type) {
            Object.defineProperty(WebSocket.prototype, 'on' + type, {
                get: function () { return this._handlers[type]; },
                set: function (value) {
                    this._handlers[type] = typeof value === 'function' ? value : null;
                },
                enumerable: true, configurable: true,
            });
        })(type);
    }

    for (var pair of [['CONNECTING', CONNECTING], ['OPEN', OPEN], ['CLOSING', CLOSING], ['CLOSED', CLOSED]]) {
        Object.defineProperty(WebSocket, pair[0], {
            value: pair[1], writable: false, enumerable: true, configurable: false,
        });
    }
    Object.defineProperty(WebSocket.prototype, Symbol.toStringTag, {
        value: 'WebSocket', configurable: true,
    });

    globalThis.WebSocket = WebSocket;
})();
