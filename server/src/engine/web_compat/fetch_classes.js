// Fetch-spec classes: Headers, Request, Response, and spec-grade Blob /
// File / FormData. Replaces the compact bootstrap versions (which live on
// only for instances created before this file runs). The network transport
// stays the existing op_fetch pipeline: fetch() is re-wrapped to accept
// Request/URL inputs and return real Response objects.
(function () {
    'use strict';
    if (typeof globalThis.Headers === 'function' &&
        typeof globalThis.Response === 'function' &&
        typeof globalThis.Blob === 'function' &&
        typeof globalThis.Blob.prototype.stream === 'function') {
        return;
    }

    var utf8encode = function (s) { return new TextEncoder().encode(s); };
    var utf8decode = function (b) { return new TextDecoder().decode(b); };

    function concatBytes(chunks) {
        var total = 0;
        for (var i = 0; i < chunks.length; i++) total += chunks[i].length;
        var out = new Uint8Array(total);
        var off = 0;
        for (var j = 0; j < chunks.length; j++) {
            out.set(chunks[j], off);
            off += chunks[j].length;
        }
        return out;
    }

    // ── Headers ─────────────────────────────────────────────────────────
    var TOKEN_RE = /^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/;

    // WebIDL ByteString: symbols throw, and every code unit must be <= 0xFF.
    function byteString(v, context) {
        if (typeof v === 'symbol') {
            throw new TypeError(context + ': Cannot convert a Symbol to a ByteString.');
        }
        var s = String(v);
        for (var i = 0; i < s.length; i++) {
            if (s.charCodeAt(i) > 0xff) {
                throw new TypeError(
                    context + ': Cannot convert argument to a ByteString because the character at index ' +
                    i + ' has a value of ' + s.charCodeAt(i) + ' which is greater than 255.');
            }
        }
        return s;
    }

    var FORBIDDEN_RESPONSE_HEADERS = ['set-cookie', 'set-cookie2'];

    function normalizeValue(value) {
        // Trim leading/trailing HTTP whitespace.
        return byteString(value, "Failed to execute 'append' on 'Headers'")
            .replace(/^[\t\n\r ]+|[\t\n\r ]+$/g, '');
    }

    function validateName(name, method) {
        name = byteString(name, "Failed to execute '" + method + "' on 'Headers'");
        if (!TOKEN_RE.test(name)) {
            throw new TypeError(
                "Failed to execute '" + method + "' on 'Headers': Invalid name");
        }
        return name.toLowerCase();
    }

    function validateValue(value, method) {
        if (/[\0\r\n]/.test(value)) {
            throw new TypeError(
                "Failed to execute '" + method + "' on 'Headers': Invalid value");
        }
        return value;
    }

    var headersData = new WeakMap();

    function hdata(h) {
        var d = headersData.get(h);
        if (!d) throw new TypeError('Illegal invocation');
        return d;
    }

    function fillHeaders(headers, init) {
        if (init === undefined) return;
        if (init === null || (typeof init !== 'object' && typeof init !== 'function')) {
            throw new TypeError(
                "Failed to construct 'Headers': The provided value is not of type '(record<ByteString, ByteString> or sequence<sequence<ByteString>>)'.");
        }
        if (typeof init[Symbol.iterator] === 'function') {
            var pairs = Array.from(init);
            for (var i = 0; i < pairs.length; i++) {
                var pair = Array.from(pairs[i]);
                if (pair.length !== 2) {
                    throw new TypeError(
                        "Failed to construct 'Headers': Invalid value");
                }
                headers.append(pair[0], pair[1]);
            }
        } else {
            // WebIDL record<K,V> conversion: [[OwnPropertyKeys]], then per
            // key [[GetOwnProperty]] followed immediately by [[Get]] — the
            // interleaving is observable through proxies.
            var keys = Reflect.ownKeys(init);
            for (var j = 0; j < keys.length; j++) {
                var desc = Reflect.getOwnPropertyDescriptor(init, keys[j]);
                if (desc && desc.enumerable) {
                    // Key converts to ByteString before [[Get]] (symbols and
                    // >0xFF code units throw here), then the value.
                    var key = byteString(keys[j], "Failed to construct 'Headers'");
                    headers.append(key, init[keys[j]]);
                }
            }
        }
    }

    class Headers {
        constructor(init) {
            headersData.set(this, { list: [], guard: 'none' });
            fillHeaders(this, init);
        }
        append(name, value) {
            if (arguments.length < 2) {
                throw new TypeError(
                    "Failed to execute 'append' on 'Headers': 2 arguments required, but only " +
                    arguments.length + ' present.');
            }
            var d = hdata(this);
            value = normalizeValue(value);
            var lower = validateName(name, 'append');
            validateValue(value, 'append');
            if (d.guard === 'response' && FORBIDDEN_RESPONSE_HEADERS.indexOf(lower) !== -1) {
                return;
            }
            d.list.push([lower, value]);
        }
        delete(name) {
            if (arguments.length < 1) {
                throw new TypeError(
                    "Failed to execute 'delete' on 'Headers': 1 argument required, but only 0 present.");
            }
            var d = hdata(this);
            var lower = validateName(name, 'delete');
            d.list = d.list.filter(function (e) { return e[0] !== lower; });
        }
        get(name) {
            if (arguments.length < 1) {
                throw new TypeError(
                    "Failed to execute 'get' on 'Headers': 1 argument required, but only 0 present.");
            }
            var d = hdata(this);
            var lower = validateName(name, 'get');
            var values = [];
            for (var i = 0; i < d.list.length; i++) {
                if (d.list[i][0] === lower) values.push(d.list[i][1]);
            }
            return values.length === 0 ? null : values.join(', ');
        }
        getSetCookie() {
            var d = hdata(this);
            var out = [];
            for (var i = 0; i < d.list.length; i++) {
                if (d.list[i][0] === 'set-cookie') out.push(d.list[i][1]);
            }
            return out;
        }
        has(name) {
            if (arguments.length < 1) {
                throw new TypeError(
                    "Failed to execute 'has' on 'Headers': 1 argument required, but only 0 present.");
            }
            var d = hdata(this);
            var lower = validateName(name, 'has');
            for (var i = 0; i < d.list.length; i++) {
                if (d.list[i][0] === lower) return true;
            }
            return false;
        }
        set(name, value) {
            if (arguments.length < 2) {
                throw new TypeError(
                    "Failed to execute 'set' on 'Headers': 2 arguments required, but only " +
                    arguments.length + ' present.');
            }
            var d = hdata(this);
            value = normalizeValue(value);
            var lower = validateName(name, 'set');
            validateValue(value, 'set');
            if (d.guard === 'response' && FORBIDDEN_RESPONSE_HEADERS.indexOf(lower) !== -1) {
                return;
            }
            var replaced = false;
            var out = [];
            for (var i = 0; i < d.list.length; i++) {
                if (d.list[i][0] === lower) {
                    if (!replaced) {
                        out.push([lower, value]);
                        replaced = true;
                    }
                } else {
                    out.push(d.list[i]);
                }
            }
            if (!replaced) out.push([lower, value]);
            d.list = out;
        }
        forEach(callback, thisArg) {
            if (typeof callback !== 'function') {
                throw new TypeError(
                    "Failed to execute 'forEach' on 'Headers': parameter 1 is not of type 'Function'.");
            }
            var entries = sortAndCombine(hdata(this));
            for (var i = 0; i < entries.length; i++) {
                callback.call(thisArg, entries[i][1], entries[i][0], this);
            }
        }
    }

    // The iteration view: names byte-lowercased and sorted; values combined
    // with ", " — except set-cookie, which stays one entry per value.
    function sortAndCombine(d) {
        var names = [];
        var seen = {};
        for (var i = 0; i < d.list.length; i++) {
            if (!seen[d.list[i][0]]) {
                seen[d.list[i][0]] = true;
                names.push(d.list[i][0]);
            }
        }
        names.sort();
        var out = [];
        for (var j = 0; j < names.length; j++) {
            var name = names[j];
            if (name === 'set-cookie') {
                for (var k = 0; k < d.list.length; k++) {
                    if (d.list[k][0] === 'set-cookie') out.push([name, d.list[k][1]]);
                }
            } else {
                var values = [];
                for (var l = 0; l < d.list.length; l++) {
                    if (d.list[l][0] === name) values.push(d.list[l][1]);
                }
                out.push([name, values.join(', ')]);
            }
        }
        return out;
    }

    var headersIterProto = Object.create(
        Object.getPrototypeOf(Object.getPrototypeOf([][Symbol.iterator]())));
    headersIterProto.next = function next() { return this.__next(); };
    Object.defineProperty(headersIterProto, Symbol.toStringTag, {
        value: 'Headers Iterator', configurable: true,
    });

    function headersIterator(target, kind) {
        var index = 0;
        var iter = Object.create(headersIterProto);
        iter.__next = function () {
            var entries = sortAndCombine(hdata(target));
            if (index >= entries.length) return { value: undefined, done: true };
            var e = entries[index++];
            var value = kind === 'key' ? e[0] : kind === 'value' ? e[1] : [e[0], e[1]];
            return { value: value, done: false };
        };
        return iter;
    }

    Headers.prototype.entries = function entries() { return headersIterator(this, 'key+value'); };
    Headers.prototype.keys = function keys() { return headersIterator(this, 'key'); };
    Headers.prototype.values = function values() { return headersIterator(this, 'value'); };
    Headers.prototype[Symbol.iterator] = Headers.prototype.entries;
    Object.defineProperty(Headers.prototype, Symbol.toStringTag, {
        value: 'Headers', configurable: true,
    });
    globalThis.Headers = Headers;

    // ── Blob / File ─────────────────────────────────────────────────────
    var blobData = new WeakMap();

    function bdata(b) {
        var d = blobData.get(b);
        if (!d) throw new TypeError('Illegal invocation');
        return d;
    }

    function normalizeType(t) {
        t = t === undefined ? '' : String(t);
        for (var i = 0; i < t.length; i++) {
            var c = t.charCodeAt(i);
            if (c < 0x20 || c > 0x7e) return '';
        }
        return t.toLowerCase();
    }

    function processBlobParts(parts, endings) {
        if (typeof parts !== 'object' || parts === null ||
            typeof parts[Symbol.iterator] !== 'function') {
            throw new TypeError(
                "Failed to construct 'Blob': The provided value cannot be converted to a sequence.");
        }
        var chunks = [];
        // Sequence conversion is lazy: each element converts to a BlobPart
        // as the iterator yields it (observable through getters).
        for (var part of parts) {
            if (part instanceof Blob) {
                chunks.push(bdata(part).bytes);
            } else if (part instanceof ArrayBuffer) {
                chunks.push(new Uint8Array(part.slice(0)));
            } else if (ArrayBuffer.isView(part)) {
                chunks.push(new Uint8Array(
                    part.buffer.slice(part.byteOffset, part.byteOffset + part.byteLength)));
            } else {
                var s = String(part);
                if (endings === 'native') {
                    s = s.replace(/\r\n|\r|\n/g, '\n');
                }
                chunks.push(utf8encode(s));
            }
        }
        return concatBytes(chunks);
    }

    class Blob {
        constructor(blobParts = undefined, options = undefined) {
            var endings = 'transparent', type = '';
            if (options !== undefined && options !== null) {
                if (typeof options !== 'object' && typeof options !== 'function') {
                    throw new TypeError(
                        "Failed to construct 'Blob': The provided value is not of type 'BlobPropertyBag'.");
                }
                if (options.endings !== undefined) {
                    endings = String(options.endings);
                    if (endings !== 'transparent' && endings !== 'native') {
                        throw new TypeError(
                            "Failed to construct 'Blob': The provided value '" + endings +
                            "' is not a valid enum value of type EndingType.");
                    }
                }
                if (options.type !== undefined) type = normalizeType(options.type);
            }
            var bytes = blobParts === undefined
                ? new Uint8Array(0)
                : processBlobParts(blobParts, endings);
            blobData.set(this, { bytes: bytes, type: type });
        }
        get size() { return bdata(this).bytes.length; }
        get type() { return bdata(this).type; }
        slice(start, end, contentType) {
            var d = bdata(this);
            var size = d.bytes.length;
            // WebIDL [Clamp] long long: round half to even.
            function clampIndex(v) {
                v = Number(v);
                if (Number.isNaN(v)) return 0;
                var f = Math.floor(v);
                if (v - f === 0.5) return f % 2 === 0 ? f : f + 1;
                return Math.round(v);
            }
            var s = start === undefined ? 0 : clampIndex(start);
            var e = end === undefined ? size : clampIndex(end);
            if (s < 0) s = Math.max(size + s, 0); else s = Math.min(s, size);
            if (e < 0) e = Math.max(size + e, 0); else e = Math.min(e, size);
            var span = Math.max(e - s, 0);
            var slice = new Blob([], { type: contentType === undefined ? '' : contentType });
            bdata(slice).bytes = d.bytes.slice(s, s + span);
            return slice;
        }
        arrayBuffer() {
            var d = bdata(this);
            var copy = d.bytes.slice(0);
            return Promise.resolve(copy.buffer);
        }
        bytes() {
            return Promise.resolve(bdata(this).bytes.slice(0));
        }
        text() {
            return Promise.resolve(utf8decode(bdata(this).bytes));
        }
        stream() {
            var bytes = bdata(this).bytes.slice(0);
            return new ReadableStream({
                type: 'bytes',
                start: function (controller) {
                    if (bytes.length > 0) controller.enqueue(bytes);
                    controller.close();
                },
            });
        }
        textStream() {
            return this.stream().pipeThrough(new TextDecoderStream());
        }
    }
    Object.defineProperty(Blob.prototype, Symbol.toStringTag, {
        value: 'Blob', configurable: true,
    });
    globalThis.Blob = Blob;

    var fileData = new WeakMap();

    class File extends Blob {
        constructor(fileBits, fileName, options) {
            if (arguments.length < 2) {
                throw new TypeError(
                    "Failed to construct 'File': 2 arguments required, but only " +
                    arguments.length + ' present.');
            }
            super(fileBits, options);
            var lastModified = Date.now();
            if (options !== undefined && options !== null &&
                (typeof options === 'object' || typeof options === 'function') &&
                options.lastModified !== undefined) {
                lastModified = Math.trunc(Number(options.lastModified)) || 0;
            }
            fileData.set(this, { name: String(fileName), lastModified: lastModified });
        }
        get name() {
            var d = fileData.get(this);
            if (!d) throw new TypeError('Illegal invocation');
            return d.name;
        }
        get lastModified() {
            var d = fileData.get(this);
            if (!d) throw new TypeError('Illegal invocation');
            return d.lastModified;
        }
        get webkitRelativePath() { return ''; }
    }
    Object.defineProperty(File.prototype, Symbol.toStringTag, {
        value: 'File', configurable: true,
    });
    globalThis.File = File;

    // ── FormData ────────────────────────────────────────────────────────
    var formDataData = new WeakMap();

    function fdata(fd) {
        var d = formDataData.get(fd);
        if (!d) throw new TypeError('Illegal invocation');
        return d;
    }

    function toFormDataValue(value, filename, hasFilename) {
        if (value instanceof Blob) {
            var name = hasFilename ? String(filename)
                : (value instanceof File ? value.name : 'blob');
            if (value instanceof File && !hasFilename) {
                return value;
            }
            var f = new File([value], name, {
                type: value.type,
                lastModified: value instanceof File ? value.lastModified : Date.now(),
            });
            return f;
        }
        if (hasFilename) {
            throw new TypeError(
                "Failed to execute 'append' on 'FormData': parameter 2 is not of type 'Blob'.");
        }
        return String(value);
    }

    class FormData {
        constructor(form) {
            if (form !== undefined) {
                throw new TypeError(
                    "Failed to construct 'FormData': parameter 1 is not of type 'HTMLFormElement'.");
            }
            formDataData.set(this, { list: [] });
        }
        append(name, value, filename) {
            if (arguments.length < 2) {
                throw new TypeError(
                    "Failed to execute 'append' on 'FormData': 2 arguments required, but only " +
                    arguments.length + ' present.');
            }
            var d = fdata(this);
            d.list.push([String(name), toFormDataValue(value, filename, arguments.length > 2)]);
        }
        delete(name) {
            if (arguments.length < 1) {
                throw new TypeError(
                    "Failed to execute 'delete' on 'FormData': 1 argument required, but only 0 present.");
            }
            var d = fdata(this);
            name = String(name);
            d.list = d.list.filter(function (e) { return e[0] !== name; });
        }
        get(name) {
            if (arguments.length < 1) {
                throw new TypeError(
                    "Failed to execute 'get' on 'FormData': 1 argument required, but only 0 present.");
            }
            var d = fdata(this);
            name = String(name);
            for (var i = 0; i < d.list.length; i++) {
                if (d.list[i][0] === name) return d.list[i][1];
            }
            return null;
        }
        getAll(name) {
            if (arguments.length < 1) {
                throw new TypeError(
                    "Failed to execute 'getAll' on 'FormData': 1 argument required, but only 0 present.");
            }
            var d = fdata(this);
            name = String(name);
            return d.list.filter(function (e) { return e[0] === name; })
                .map(function (e) { return e[1]; });
        }
        has(name) {
            if (arguments.length < 1) {
                throw new TypeError(
                    "Failed to execute 'has' on 'FormData': 1 argument required, but only 0 present.");
            }
            var d = fdata(this);
            name = String(name);
            for (var i = 0; i < d.list.length; i++) {
                if (d.list[i][0] === name) return true;
            }
            return false;
        }
        set(name, value, filename) {
            if (arguments.length < 2) {
                throw new TypeError(
                    "Failed to execute 'set' on 'FormData': 2 arguments required, but only " +
                    arguments.length + ' present.');
            }
            var d = fdata(this);
            name = String(name);
            var entry = [name, toFormDataValue(value, filename, arguments.length > 2)];
            var replaced = false;
            var out = [];
            for (var i = 0; i < d.list.length; i++) {
                if (d.list[i][0] === name) {
                    if (!replaced) {
                        out.push(entry);
                        replaced = true;
                    }
                } else {
                    out.push(d.list[i]);
                }
            }
            if (!replaced) out.push(entry);
            d.list = out;
        }
        forEach(callback, thisArg) {
            if (typeof callback !== 'function') {
                throw new TypeError(
                    "Failed to execute 'forEach' on 'FormData': parameter 1 is not of type 'Function'.");
            }
            var d = fdata(this);
            for (var i = 0; i < d.list.length; i++) {
                callback.call(thisArg, d.list[i][1], d.list[i][0], this);
            }
        }
        // Compatibility with the op_fetch body pipeline: multipart/form-data
        // serialization as a latin1 byte-string plus boundary.
        _serialize() {
            var d = fdata(this);
            var boundary = '----McpV8FormBoundary';
            var rand = new Uint8Array(12);
            if (globalThis.crypto && crypto.getRandomValues) {
                crypto.getRandomValues(rand);
            } else {
                for (var r = 0; r < rand.length; r++) rand[r] = (Math.random() * 256) | 0;
            }
            for (var b = 0; b < rand.length; b++) {
                boundary += (rand[b] & 0x3f).toString(36);
            }
            var latin1 = function (bytes) {
                var out = '';
                for (var i = 0; i < bytes.length; i++) out += String.fromCharCode(bytes[i]);
                return out;
            };
            var escapeName = function (n) {
                return n.replace(/\r?\n|\r/g, '\r\n')
                    .replace(/\n/g, '%0A').replace(/\r/g, '%0D').replace(/"/g, '%22');
            };
            var body = '';
            for (var i = 0; i < d.list.length; i++) {
                var name = d.list[i][0], value = d.list[i][1];
                body += '--' + boundary + '\r\n';
                if (value instanceof File) {
                    body += 'Content-Disposition: form-data; name="' + escapeName(name) +
                        '"; filename="' + escapeName(value.name) + '"\r\n';
                    body += 'Content-Type: ' + (value.type || 'application/octet-stream') + '\r\n\r\n';
                    body += latin1(bdata(value).bytes) + '\r\n';
                } else {
                    body += 'Content-Disposition: form-data; name="' + escapeName(name) + '"\r\n\r\n';
                    body += latin1(utf8encode(String(value))) + '\r\n';
                }
            }
            body += '--' + boundary + '--\r\n';
            return { boundary: boundary, body: body };
        }
    }

    var fdIterProto = Object.create(
        Object.getPrototypeOf(Object.getPrototypeOf([][Symbol.iterator]())));
    fdIterProto.next = function next() { return this.__next(); };
    Object.defineProperty(fdIterProto, Symbol.toStringTag, {
        value: 'FormData Iterator', configurable: true,
    });

    function fdIterator(target, kind) {
        var index = 0;
        var iter = Object.create(fdIterProto);
        iter.__next = function () {
            var d = fdata(target);
            if (index >= d.list.length) return { value: undefined, done: true };
            var e = d.list[index++];
            var value = kind === 'key' ? e[0] : kind === 'value' ? e[1] : [e[0], e[1]];
            return { value: value, done: false };
        };
        return iter;
    }

    FormData.prototype.entries = function entries() { return fdIterator(this, 'key+value'); };
    FormData.prototype.keys = function keys() { return fdIterator(this, 'key'); };
    FormData.prototype.values = function values() { return fdIterator(this, 'value'); };
    FormData.prototype[Symbol.iterator] = FormData.prototype.entries;
    Object.defineProperty(FormData.prototype, Symbol.toStringTag, {
        value: 'FormData', configurable: true,
    });
    globalThis.FormData = FormData;

    // ── Body mixin ──────────────────────────────────────────────────────
    // Each Request/Response holds { bytes: Uint8Array|null, stream:
    // ReadableStream|null, used: bool }. Bytes-backed bodies lazily create
    // their stream view.
    function makeBodyState(bytes, stream) {
        return { bytes: bytes || null, stream: stream || null, used: false };
    }

    function consumeBody(state, ctor) {
        if (state.used || (state.stream && (state.stream.locked || isDisturbed(state.stream)))) {
            return Promise.reject(new TypeError('Body has already been consumed.'));
        }
        state.used = true;
        if (state.bytes !== null) {
            return Promise.resolve(state.bytes.slice(0));
        }
        if (state.stream === null) {
            return Promise.resolve(new Uint8Array(0));
        }
        var reader = state.stream.getReader();
        var chunks = [];
        return (function pump() {
            return reader.read().then(function (r) {
                if (r.done) return concatBytes(chunks);
                if (!(r.value instanceof Uint8Array)) {
                    throw new TypeError('Received non-Uint8Array chunk from body stream.');
                }
                chunks.push(r.value);
                return pump();
            });
        })();
    }

    function isDisturbed(stream) {
        // The polyfill offers no introspection; track via our own flag only.
        return false;
    }

    function defineBodyMixin(proto, getState, getMime) {
        Object.defineProperty(proto, 'body', {
            enumerable: true,
            configurable: true,
            get: function () {
                var state = getState(this);
                if (state.bytes === null && state.stream === null) return null;
                if (state.stream === null) {
                    var bytes = state.bytes;
                    state.stream = new ReadableStream({
                        type: 'bytes',
                        start: function (controller) {
                            if (bytes.length > 0) controller.enqueue(bytes.slice(0));
                            controller.close();
                        },
                    });
                }
                return state.stream;
            },
        });
        Object.defineProperty(proto, 'bodyUsed', {
            enumerable: true,
            configurable: true,
            get: function () {
                var state = getState(this);
                return state.used || (state.stream !== null && state.stream.locked);
            },
        });
        proto.arrayBuffer = function arrayBuffer() {
            var self_ = this;
            try {
                return consumeBody(getState(this)).then(function (b) { return b.buffer; });
            } catch (e) { return Promise.reject(e); }
        };
        proto.bytes = function bytes() {
            try {
                return consumeBody(getState(this));
            } catch (e) { return Promise.reject(e); }
        };
        proto.text = function text() {
            try {
                return consumeBody(getState(this)).then(utf8decode);
            } catch (e) { return Promise.reject(e); }
        };
        proto.json = function json() {
            try {
                return consumeBody(getState(this)).then(function (b) {
                    return JSON.parse(utf8decode(b));
                });
            } catch (e) { return Promise.reject(e); }
        };
        proto.blob = function blob() {
            var self_ = this;
            try {
                return consumeBody(getState(this)).then(function (b) {
                    return new Blob([b], { type: getMime(self_) || '' });
                });
            } catch (e) { return Promise.reject(e); }
        };
        proto.formData = function formData() {
            var self_ = this;
            try {
                return consumeBody(getState(this)).then(function (b) {
                    var mime = (getMime(self_) || '').toLowerCase();
                    if (mime.indexOf('application/x-www-form-urlencoded') === 0) {
                        var fd = new FormData();
                        var params = new URLSearchParams(utf8decode(b));
                        params.forEach(function (v, k) { fd.append(k, v); });
                        return fd;
                    }
                    throw new TypeError('Failed to parse body as FormData.');
                });
            } catch (e) { return Promise.reject(e); }
        };
    }

    // Extract [bytes|stream, contentType] from a BodyInit.
    function extractBody(body, what) {
        if (body === undefined || body === null) return { bytes: null, stream: null, type: null };
        if (body instanceof Blob) {
            return { bytes: bdata(body).bytes.slice(0), stream: null, type: body.type || null };
        }
        if (typeof ReadableStream === 'function' && body instanceof ReadableStream) {
            return { bytes: null, stream: body, type: null };
        }
        if (body instanceof ArrayBuffer) {
            return { bytes: new Uint8Array(body.slice(0)), stream: null, type: null };
        }
        if (ArrayBuffer.isView(body)) {
            return {
                bytes: new Uint8Array(
                    body.buffer.slice(body.byteOffset, body.byteOffset + body.byteLength)),
                stream: null, type: null,
            };
        }
        if (typeof URLSearchParams === 'function' && body instanceof URLSearchParams) {
            return {
                bytes: utf8encode(body.toString()), stream: null,
                type: 'application/x-www-form-urlencoded;charset=UTF-8',
            };
        }
        if (typeof FormData === 'function' && body instanceof FormData) {
            var ser = body._serialize();
            var raw = new Uint8Array(ser.body.length);
            for (var i = 0; i < ser.body.length; i++) raw[i] = ser.body.charCodeAt(i) & 0xff;
            return {
                bytes: raw, stream: null,
                type: 'multipart/form-data; boundary=' + ser.boundary,
            };
        }
        return { bytes: utf8encode(String(body)), stream: null, type: 'text/plain;charset=UTF-8' };
    }

    // ── Request ─────────────────────────────────────────────────────────
    var requestData = new WeakMap();

    function rqdata(r) {
        var d = requestData.get(r);
        if (!d) throw new TypeError('Illegal invocation');
        return d;
    }

    var METHOD_TOKEN = TOKEN_RE;
    var FORBIDDEN_METHODS = ['CONNECT', 'TRACE', 'TRACK'];

    function normalizeMethod(m) {
        m = String(m);
        if (!METHOD_TOKEN.test(m)) {
            throw new TypeError("'" + m + "' is not a valid HTTP method.");
        }
        if (FORBIDDEN_METHODS.indexOf(m.toUpperCase()) !== -1) {
            throw new TypeError("'" + m + "' HTTP method is unsupported.");
        }
        var upper = m.toUpperCase();
        if (['DELETE', 'GET', 'HEAD', 'OPTIONS', 'POST', 'PUT'].indexOf(upper) !== -1) {
            return upper;
        }
        return m;
    }

    class Request {
        constructor(input, init) {
            if (arguments.length < 1) {
                throw new TypeError(
                    "Failed to construct 'Request': 1 argument required, but only 0 present.");
            }
            init = init === undefined || init === null ? {} : init;
            var url, method = 'GET', headers = null, bodyInit, signal = null;
            if (input instanceof Request) {
                var src = rqdata(input);
                url = src.url;
                method = src.method;
                headers = new Headers();
                hdata(headers).list = hdata(src.headers).list.slice();
                signal = src.signal;
                if (src.body.bytes !== null || src.body.stream !== null) {
                    if (src.body.used) {
                        throw new TypeError(
                            "Failed to construct 'Request': Request body is already used.");
                    }
                    bodyInit = { bytes: src.body.bytes, stream: src.body.stream, type: null };
                    src.body.used = true;
                }
            } else {
                var parsed = new URL(String(input)); // throws TypeError on invalid
                if (parsed.username || parsed.password) {
                    throw new TypeError(
                        "Failed to construct 'Request': Request cannot be constructed from a URL that includes credentials.");
                }
                url = parsed.href;
            }
            if (init.method !== undefined) method = normalizeMethod(init.method);
            if (init.headers !== undefined) {
                headers = new Headers(init.headers);
            } else if (headers === null) {
                headers = new Headers();
            }
            if (init.signal !== undefined && init.signal !== null) {
                if (!(init.signal instanceof AbortSignal)) {
                    throw new TypeError(
                        "Failed to construct 'Request': member signal is not of type AbortSignal.");
                }
                signal = init.signal;
            }
            if (signal === null) {
                signal = new AbortController().signal;
            }
            var body = { bytes: null, stream: null };
            if (init.body !== undefined && init.body !== null) {
                if (method === 'GET' || method === 'HEAD') {
                    throw new TypeError('Request with GET/HEAD method cannot have body.');
                }
                var extracted = extractBody(init.body, 'Request');
                body.bytes = extracted.bytes;
                body.stream = extracted.stream;
                if (extracted.type !== null && !headers.has('content-type')) {
                    headers.set('content-type', extracted.type);
                }
            } else if (bodyInit) {
                body.bytes = bodyInit.bytes;
                body.stream = bodyInit.stream;
            }
            body.used = false;
            requestData.set(this, {
                url: url, method: method, headers: headers, signal: signal,
                body: body,
                mode: init.mode !== undefined ? String(init.mode) : 'cors',
                credentials: init.credentials !== undefined ? String(init.credentials) : 'same-origin',
                cache: init.cache !== undefined ? String(init.cache) : 'default',
                redirect: init.redirect !== undefined ? String(init.redirect) : 'follow',
                referrer: init.referrer !== undefined ? String(init.referrer) : 'about:client',
                referrerPolicy: init.referrerPolicy !== undefined ? String(init.referrerPolicy) : '',
                integrity: init.integrity !== undefined ? String(init.integrity) : '',
                keepalive: !!init.keepalive,
            });
        }
        get url() { return rqdata(this).url; }
        get method() { return rqdata(this).method; }
        get headers() { return rqdata(this).headers; }
        get destination() { return ''; }
        get referrer() { return rqdata(this).referrer; }
        get referrerPolicy() { return rqdata(this).referrerPolicy; }
        get mode() { return rqdata(this).mode; }
        get credentials() { return rqdata(this).credentials; }
        get cache() { return rqdata(this).cache; }
        get redirect() { return rqdata(this).redirect; }
        get integrity() { return rqdata(this).integrity; }
        get keepalive() { return rqdata(this).keepalive; }
        get isReloadNavigation() { return false; }
        get isHistoryNavigation() { return false; }
        get signal() { return rqdata(this).signal; }
        get duplex() { return 'half'; }
        clone() {
            var d = rqdata(this);
            if (d.body.used || (d.body.stream && d.body.stream.locked)) {
                throw new TypeError(
                    "Failed to execute 'clone' on 'Request': Request body is already used.");
            }
            var copy = new Request(d.url === '' ? 'about:blank' : d.url);
            var cd = rqdata(copy);
            cd.url = d.url;
            cd.method = d.method;
            hdata(cd.headers).list = hdata(d.headers).list.slice();
            cd.signal = d.signal;
            cd.mode = d.mode; cd.credentials = d.credentials; cd.cache = d.cache;
            cd.redirect = d.redirect; cd.referrer = d.referrer;
            cd.referrerPolicy = d.referrerPolicy; cd.integrity = d.integrity;
            cd.keepalive = d.keepalive;
            cd.body = {
                bytes: d.body.bytes === null ? null : d.body.bytes.slice(0),
                stream: null, used: false,
            };
            if (d.body.stream !== null) {
                var tees = d.body.stream.tee();
                d.body.stream = tees[0];
                cd.body.stream = tees[1];
            }
            return copy;
        }
    }
    defineBodyMixin(Request.prototype,
        function (r) { return rqdata(r).body; },
        function (r) { return rqdata(r).headers.get('content-type'); });
    Object.defineProperty(Request.prototype, Symbol.toStringTag, {
        value: 'Request', configurable: true,
    });
    globalThis.Request = Request;

    // ── Response ────────────────────────────────────────────────────────
    var responseData = new WeakMap();

    function rsdata(r) {
        var d = responseData.get(r);
        if (!d) throw new TypeError('Illegal invocation');
        return d;
    }

    var REDIRECT_STATUSES = [301, 302, 303, 307, 308];

    class Response {
        constructor(body, init) {
            init = init === undefined || init === null ? {} : init;
            var status = init.status !== undefined ? Number(init.status) | 0 : 200;
            if (status < 200 || status > 599) {
                throw new RangeError(
                    "Failed to construct 'Response': The status provided (" + status +
                    ') is outside the range [200, 599].');
            }
            var statusText = init.statusText !== undefined ? String(init.statusText) : '';
            if (/[\r\n]/.test(statusText)) {
                throw new TypeError(
                    "Failed to construct 'Response': Invalid statusText");
            }
            var headers = new Headers();
            hdata(headers).guard = 'response';
            fillHeaders(headers, init.headers);
            var state = { bytes: null, stream: null, used: false };
            if (body !== undefined && body !== null) {
                if (status === 204 || status === 205 || status === 304) {
                    throw new TypeError(
                        "Failed to construct 'Response': Response with null body status cannot have body");
                }
                var extracted = extractBody(body, 'Response');
                state.bytes = extracted.bytes;
                state.stream = extracted.stream;
                if (extracted.type !== null && !headers.has('content-type')) {
                    headers.set('content-type', extracted.type);
                }
            }
            responseData.set(this, {
                type: 'default', url: '', redirected: false,
                status: status, statusText: statusText, headers: headers,
                body: state,
            });
        }
        static error() {
            var r = new Response();
            var d = rsdata(r);
            d.type = 'error';
            d.status = 0;
            d.statusText = '';
            hdata(d.headers).list = [];
            return r;
        }
        static redirect(url, status) {
            var parsed = new URL(String(url)); // throws on invalid
            status = status === undefined ? 302 : Number(status) | 0;
            if (REDIRECT_STATUSES.indexOf(status) === -1) {
                throw new RangeError("Failed to execute 'redirect' on 'Response': Invalid status code");
            }
            var r = new Response(undefined, { status: status });
            rsdata(r).headers.set('location', parsed.href);
            return r;
        }
        static json(data, init) {
            var text = JSON.stringify(data);
            if (text === undefined) {
                throw new TypeError('The data is not JSON serializable');
            }
            init = init === undefined || init === null ? {} : init;
            var headers = new Headers(init.headers);
            if (!headers.has('content-type')) {
                headers.set('content-type', 'application/json');
            }
            return new Response(text, {
                status: init.status, statusText: init.statusText, headers: headers,
            });
        }
        get type() { return rsdata(this).type; }
        get url() { return rsdata(this).url; }
        get redirected() { return rsdata(this).redirected; }
        get status() { return rsdata(this).status; }
        get ok() {
            var s = rsdata(this).status;
            return s >= 200 && s <= 299;
        }
        get statusText() { return rsdata(this).statusText; }
        get headers() { return rsdata(this).headers; }
        clone() {
            var d = rsdata(this);
            if (d.body.used || (d.body.stream && d.body.stream.locked)) {
                throw new TypeError(
                    "Failed to execute 'clone' on 'Response': Response body is already used.");
            }
            var copy = new Response();
            var cd = rsdata(copy);
            cd.type = d.type; cd.url = d.url; cd.redirected = d.redirected;
            cd.status = d.status; cd.statusText = d.statusText;
            hdata(cd.headers).list = hdata(d.headers).list.slice();
            cd.body = {
                bytes: d.body.bytes === null ? null : d.body.bytes.slice(0),
                stream: null, used: false,
            };
            if (d.body.stream !== null) {
                var tees = d.body.stream.tee();
                d.body.stream = tees[0];
                cd.body.stream = tees[1];
            }
            return copy;
        }
    }
    defineBodyMixin(Response.prototype,
        function (r) { return rsdata(r).body; },
        function (r) { return rsdata(r).headers.get('content-type'); });
    Object.defineProperty(Response.prototype, Symbol.toStringTag, {
        value: 'Response', configurable: true,
    });
    globalThis.Response = Response;

    // ── fetch() surface upgrade ─────────────────────────────────────────
    // The op_fetch transport (installed earlier) only accepts a URL string,
    // a plain-object header map, and Uint8Array-ish bodies, and returns a
    // plain object. Wrap it to speak Request/Response.
    var coreFetch = globalThis.fetch;
    if (typeof coreFetch === 'function') {
        globalThis.fetch = function fetch(input, init) {
            try {
                var request = input instanceof Request && init === undefined
                    ? input
                    : new Request(input, init);
                var rd = rqdata(request);
                if (rd.signal && rd.signal.aborted) {
                    return Promise.reject(
                        rd.signal.reason !== undefined ? rd.signal.reason
                            : new DOMException('The operation was aborted.', 'AbortError'));
                }
                var headerObj = {};
                var list = hdata(rd.headers).list;
                for (var i = 0; i < list.length; i++) {
                    headerObj[list[i][0]] = rd.headers.get(list[i][0]);
                }
                var prepare;
                if (rd.body.bytes !== null) {
                    prepare = Promise.resolve(rd.body.bytes);
                } else if (rd.body.stream !== null) {
                    prepare = consumeBody(rd.body);
                } else {
                    prepare = Promise.resolve(undefined);
                }
                return prepare.then(function (bytes) {
                    return coreFetch(rd.url, {
                        method: rd.method,
                        headers: headerObj,
                        body: bytes,
                    });
                }).then(function (raw) {
                    return raw.bytes().then(function (respBytes) {
                        var resp = new Response();
                        var sd = rsdata(resp);
                        sd.status = raw.status;
                        sd.statusText = raw.statusText || '';
                        sd.url = raw.url || rd.url;
                        sd.redirected = !!raw.redirected;
                        sd.type = 'basic';
                        var rawHeaders = raw.headers;
                        if (rawHeaders && typeof rawHeaders.forEach === 'function') {
                            rawHeaders.forEach(function (v, k) {
                                try { sd.headers.append(k, v); } catch (_) { /* skip invalid */ }
                            });
                        }
                        sd.body = makeBodyState(respBytes, null);
                        return resp;
                    });
                });
            } catch (e) {
                return Promise.reject(e);
            }
        };
    }
})();
