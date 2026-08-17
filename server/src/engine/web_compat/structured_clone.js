// structuredClone + MessageChannel/MessagePort.
//
// Implements the HTML structured clone algorithm in pure JS for the value
// types that exist in this runtime: primitives, plain objects and class
// instances (cloned as plain objects, prototype not preserved — per spec),
// Array (holes preserved), Map, Set, Date, RegExp, ArrayBuffer (+transfer
// via ArrayBuffer.prototype.transfer), TypedArrays/DataView (shared-buffer
// identity preserved), Error subclasses, DOMException, Boolean/Number/
// String/BigInt wrappers, and Blob/File when present. Symbols, functions,
// and objects with unserializable internal slots throw DataCloneError.
(function () {
    'use strict';
    if (typeof globalThis.structuredClone === 'function' &&
        typeof globalThis.MessageChannel === 'function') {
        return;
    }

    var TypedArray = Object.getPrototypeOf(Uint8Array);

    function cloneError(value) {
        var desc;
        try {
            desc = typeof value === 'symbol' ? 'Symbol' :
                typeof value === 'function' ? (value.name || 'function') :
                Object.prototype.toString.call(value);
        } catch (_) { desc = 'value'; }
        return new DOMException(desc + ' could not be cloned.', 'DataCloneError');
    }

    var ERROR_TYPES = {
        Error: Error, EvalError: EvalError, RangeError: RangeError,
        ReferenceError: ReferenceError, SyntaxError: SyntaxError,
        TypeError: TypeError, URIError: URIError,
    };
    if (typeof AggregateError === 'function') ERROR_TYPES.AggregateError = AggregateError;

    function cloneValue(value, memo, transferMap) {
        switch (typeof value) {
            case 'undefined': case 'boolean': case 'number':
            case 'string': case 'bigint':
                return value;
            case 'symbol': case 'function':
                throw cloneError(value);
        }
        if (value === null) return null;

        if (memo.has(value)) return memo.get(value);
        if (transferMap && transferMap.has(value)) {
            var transferred = transferMap.get(value);
            memo.set(value, transferred);
            return transferred;
        }

        var out;

        if (value instanceof Date) {
            out = new Date(value.getTime());
            memo.set(value, out);
            return out;
        }
        if (value instanceof RegExp) {
            out = new RegExp(value.source, value.flags);
            memo.set(value, out);
            return out;
        }
        if (value instanceof ArrayBuffer) {
            if (typeof value.detached === 'boolean' && value.detached) {
                throw cloneError(value);
            }
            if (value.resizable) {
                out = new ArrayBuffer(value.byteLength, { maxByteLength: value.maxByteLength });
                new Uint8Array(out).set(new Uint8Array(value));
            } else {
                out = value.slice(0);
            }
            memo.set(value, out);
            return out;
        }
        if (typeof SharedArrayBuffer === 'function' && value instanceof SharedArrayBuffer) {
            // Same-realm clone of a SAB shares the memory block.
            memo.set(value, value);
            return value;
        }
        if (value instanceof TypedArray || value instanceof DataView) {
            var buf = cloneValue(value.buffer, memo, transferMap);
            out = value instanceof DataView
                ? new DataView(buf, value.byteOffset, value.byteLength)
                : new value.constructor(buf, value.byteOffset, value.length);
            memo.set(value, out);
            return out;
        }
        if (value instanceof Map) {
            out = new Map();
            memo.set(value, out);
            value.forEach(function (v, k) {
                out.set(cloneValue(k, memo, transferMap), cloneValue(v, memo, transferMap));
            });
            return out;
        }
        if (value instanceof Set) {
            out = new Set();
            memo.set(value, out);
            value.forEach(function (v) {
                out.add(cloneValue(v, memo, transferMap));
            });
            return out;
        }
        if (typeof DOMException === 'function' && value instanceof DOMException) {
            out = new DOMException(value.message, value.name);
            memo.set(value, out);
            return out;
        }
        if (value instanceof Error) {
            var Ctor = ERROR_TYPES[value.name] || Error;
            // An empty message stays a *missing* own property on the clone.
            out = value.message === '' || value.message === undefined
                ? new Ctor() : new Ctor(value.message);
            memo.set(value, out);
            try {
                if ('stack' in value) out.stack = value.stack;
                if ('cause' in value) out.cause = cloneValue(value.cause, memo, transferMap);
                if (Ctor === ERROR_TYPES.AggregateError && Array.isArray(value.errors)) {
                    out.errors = cloneValue(value.errors, memo, transferMap);
                }
            } catch (_) { /* stack/cause copying is best-effort */ }
            return out;
        }
        if (typeof Blob === 'function' && value instanceof Blob) {
            var isFile = typeof File === 'function' && value instanceof File;
            out = isFile
                ? new File([value], value.name, { type: value.type, lastModified: value.lastModified })
                : value.slice(0, value.size, value.type);
            memo.set(value, out);
            return out;
        }
        // Wrapper objects.
        var tag = Object.prototype.toString.call(value);
        if (tag === '[object Boolean]') { out = Object(Boolean.prototype.valueOf.call(value)); memo.set(value, out); return out; }
        if (tag === '[object Number]') { out = Object(Number.prototype.valueOf.call(value)); memo.set(value, out); return out; }
        if (tag === '[object String]') { out = Object(String.prototype.valueOf.call(value)); memo.set(value, out); return out; }
        if (tag === '[object BigInt]') { out = Object(BigInt.prototype.valueOf.call(value)); memo.set(value, out); return out; }

        // Unserializable exotic/platform objects.
        if (value instanceof Promise ||
            (typeof WeakMap === 'function' && value instanceof WeakMap) ||
            (typeof WeakSet === 'function' && value instanceof WeakSet) ||
            (typeof WeakRef === 'function' && value instanceof WeakRef) ||
            (typeof MessagePort === 'function' && value instanceof MessagePort)) {
            throw cloneError(value);
        }

        // Platform objects (Event, EventTarget, URL, Headers, ...) are not
        // serializable unless special-cased above. Heuristic: a prototype
        // below Object.prototype carrying its own Symbol.toStringTag is a
        // branded platform (or platform-like) class.
        if (!Array.isArray(value)) {
            var proto = Object.getPrototypeOf(value);
            while (proto !== null && proto !== Object.prototype) {
                if (Object.prototype.hasOwnProperty.call(proto, Symbol.toStringTag)) {
                    throw cloneError(value);
                }
                proto = Object.getPrototypeOf(proto);
            }
        }

        if (Array.isArray(value)) {
            out = new Array(value.length);
            memo.set(value, out);
            var keys = Object.keys(value);
            for (var i = 0; i < keys.length; i++) {
                out[keys[i]] = cloneValue(value[keys[i]], memo, transferMap);
            }
            return out;
        }

        // Ordinary objects (including class instances): own enumerable
        // string-keyed properties, prototype not preserved.
        out = {};
        memo.set(value, out);
        var okeys = Object.keys(value);
        for (var j = 0; j < okeys.length; j++) {
            out[okeys[j]] = cloneValue(value[okeys[j]], memo, transferMap);
        }
        return out;
    }

    globalThis.structuredClone = function structuredClone(value, options) {
        if (arguments.length < 1) {
            throw new TypeError(
                "Failed to execute 'structuredClone': 1 argument required, but only 0 present.");
        }
        var transferMap = null;
        if (options !== undefined && options !== null) {
            if (typeof options !== 'object' && typeof options !== 'function') {
                throw new TypeError(
                    "Failed to execute 'structuredClone': The provided value is not of type 'StructuredSerializeOptions'.");
            }
            var transfer = options.transfer;
            if (transfer !== undefined) {
                var list = Array.from(transfer);
                transferMap = new Map();
                for (var i = 0; i < list.length; i++) {
                    var t = list[i];
                    if (transferMap.has(t)) {
                        throw new DOMException(
                            'Transfer list contains duplicate entries.', 'DataCloneError');
                    }
                    if (t instanceof ArrayBuffer) {
                        if (typeof t.transfer !== 'function') {
                            throw cloneError(t);
                        }
                        transferMap.set(t, t.transfer());
                    } else {
                        throw cloneError(t);
                    }
                }
            }
        }
        return cloneValue(value, new Map(), transferMap);
    };

    // ── MessageChannel / MessagePort ────────────────────────────────────
    var internal = globalThis.__webCompatInternal;
    var portData = new WeakMap();
    var allowPort = false;

    function pdata(port) {
        var d = portData.get(port);
        if (!d) throw new TypeError('Illegal invocation');
        return d;
    }

    function deliver(port, data, ports) {
        var ev = new MessageEvent('message', { data: data, ports: ports || [] });
        var ed = internal.initEventData.get(ev);
        if (ed) ed.isTrusted = true;
        internal.dispatchInternal(port, ev);
    }

    function flush(port) {
        var d = pdata(port);
        if (!d.enabled || d.closed) return;
        while (d.queue.length > 0) {
            var msg = d.queue.shift();
            deliver(port, msg.data, msg.ports);
        }
    }

    class MessagePort extends EventTarget {
        constructor() {
            if (!allowPort) throw new TypeError('Illegal constructor');
            super();
            portData.set(this, {
                entangled: null, queue: [], enabled: false, closed: false,
            });
        }
        postMessage(message, transferOrOptions) {
            var d = pdata(this);
            var transfer;
            if (Array.isArray(transferOrOptions)) {
                transfer = transferOrOptions;
            } else if (transferOrOptions && typeof transferOrOptions === 'object') {
                transfer = transferOrOptions.transfer;
            }
            var data = globalThis.structuredClone(
                message, transfer ? { transfer: transfer } : undefined);
            var target = d.entangled;
            if (!target || d.closed) return;
            var td = pdata(target);
            if (td.closed) return;
            // Delivery is a task, so handlers attached later in the same
            // synchronous block still receive the message.
            setTimeout(function () {
                if (td.closed) return;
                if (td.enabled) {
                    deliver(target, data, []);
                } else {
                    td.queue.push({ data: data, ports: [] });
                }
            }, 0);
        }
        start() {
            var d = pdata(this);
            if (d.enabled) return;
            d.enabled = true;
            var self_ = this;
            setTimeout(function () { flush(self_); }, 0);
        }
        close() {
            var d = pdata(this);
            d.closed = true;
            d.queue.length = 0;
        }
    }
    Object.defineProperty(MessagePort.prototype, Symbol.toStringTag, {
        value: 'MessagePort', configurable: true,
    });
    internal.defineEventHandler(MessagePort.prototype, 'onmessage');
    internal.defineEventHandler(MessagePort.prototype, 'onmessageerror');
    // Setting onmessage enables the port (spec behavior).
    (function () {
        var desc = Object.getOwnPropertyDescriptor(MessagePort.prototype, 'onmessage');
        Object.defineProperty(MessagePort.prototype, 'onmessage', {
            enumerable: true,
            configurable: true,
            get: desc.get,
            set: function (v) {
                desc.set.call(this, v);
                this.start();
            },
        });
    })();
    globalThis.MessagePort = MessagePort;

    var channelData = new WeakMap();
    class MessageChannel {
        constructor() {
            allowPort = true;
            var p1, p2;
            try {
                p1 = new MessagePort();
                p2 = new MessagePort();
            } finally {
                allowPort = false;
            }
            pdata(p1).entangled = p2;
            pdata(p2).entangled = p1;
            channelData.set(this, [p1, p2]);
        }
        get port1() {
            var ports = channelData.get(this);
            if (!ports) throw new TypeError('Illegal invocation');
            return ports[0];
        }
        get port2() {
            var ports = channelData.get(this);
            if (!ports) throw new TypeError('Illegal invocation');
            return ports[1];
        }
    }
    Object.defineProperty(MessageChannel.prototype, Symbol.toStringTag, {
        value: 'MessageChannel', configurable: true,
    });
    globalThis.MessageChannel = MessageChannel;
})();
