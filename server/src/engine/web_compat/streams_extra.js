// TextEncoderStream / TextDecoderStream, built on TransformStream (from
// the vendored web-streams-polyfill) and the encoding layer.
(function () {
    'use strict';
    if (typeof globalThis.TextEncoderStream === 'function' &&
        typeof globalThis.TextDecoderStream === 'function') {
        return;
    }

    // The polyfill's async-iterator prototype chain ends at
    // Object.prototype; the spec puts %AsyncIteratorPrototype% there.
    try {
        var AsyncIteratorPrototype = Object.getPrototypeOf(
            Object.getPrototypeOf(async function* () {}.prototype));
        var probeStream = new ReadableStream({ start: function (c) { c.close(); } });
        var iterProto = Object.getPrototypeOf(probeStream.values());
        if (Object.getPrototypeOf(iterProto) === Object.prototype) {
            Object.setPrototypeOf(iterProto, AsyncIteratorPrototype);
        }
    } catch (_) { /* best effort */ }

    // The spec's ReadableStream.from rejects strings even though they are
    // iterable; the vendored polyfill accepts them.
    if (typeof ReadableStream === 'function' &&
        typeof ReadableStream.from === 'function') {
        var origFrom = ReadableStream.from;
        Object.defineProperty(ReadableStream, 'from', {
            value: function from(asyncIterable) {
                if (typeof asyncIterable === 'string') {
                    throw new TypeError(
                        "Failed to execute 'from' on 'ReadableStream': a string is not a valid iterable.");
                }
                return origFrom.call(this, asyncIterable);
            },
            writable: true,
            configurable: true,
        });
    }

    var tesData = new WeakMap();

    class TextEncoderStream {
        constructor() {
            var pendingLead = '';
            var encoder = new TextEncoder();
            var transform = new TransformStream({
                transform: function (chunk, controller) {
                    var s = pendingLead + String(chunk);
                    pendingLead = '';
                    if (s.length > 0) {
                        // Hold a trailing lead surrogate: its pair may arrive
                        // in the next chunk.
                        var last = s.charCodeAt(s.length - 1);
                        if (last >= 0xd800 && last <= 0xdbff) {
                            pendingLead = s.slice(-1);
                            s = s.slice(0, -1);
                        }
                    }
                    if (s.length > 0) controller.enqueue(encoder.encode(s));
                },
                flush: function (controller) {
                    if (pendingLead.length > 0) {
                        controller.enqueue(encoder.encode(pendingLead));
                    }
                },
            });
            tesData.set(this, { transform: transform, encoding: 'utf-8' });
        }
        get encoding() {
            var d = tesData.get(this);
            if (!d) throw new TypeError('Illegal invocation');
            return d.encoding;
        }
        get readable() {
            var d = tesData.get(this);
            if (!d) throw new TypeError('Illegal invocation');
            return d.transform.readable;
        }
        get writable() {
            var d = tesData.get(this);
            if (!d) throw new TypeError('Illegal invocation');
            return d.transform.writable;
        }
    }
    Object.defineProperty(TextEncoderStream.prototype, Symbol.toStringTag, {
        value: 'TextEncoderStream', configurable: true,
    });
    globalThis.TextEncoderStream = TextEncoderStream;

    var tdsData = new WeakMap();

    class TextDecoderStream {
        constructor(label, options) {
            var decoder = new TextDecoder(label, options);
            var transform = new TransformStream({
                transform: function (chunk, controller) {
                    var out = decoder.decode(chunk, { stream: true });
                    if (out.length > 0) controller.enqueue(out);
                },
                flush: function (controller) {
                    var out = decoder.decode();
                    if (out.length > 0) controller.enqueue(out);
                },
            });
            tdsData.set(this, { transform: transform, decoder: decoder });
        }
        get encoding() {
            var d = tdsData.get(this);
            if (!d) throw new TypeError('Illegal invocation');
            return d.decoder.encoding;
        }
        get fatal() {
            var d = tdsData.get(this);
            if (!d) throw new TypeError('Illegal invocation');
            return d.decoder.fatal;
        }
        get ignoreBOM() {
            var d = tdsData.get(this);
            if (!d) throw new TypeError('Illegal invocation');
            return d.decoder.ignoreBOM;
        }
        get readable() {
            var d = tdsData.get(this);
            if (!d) throw new TypeError('Illegal invocation');
            return d.transform.readable;
        }
        get writable() {
            var d = tdsData.get(this);
            if (!d) throw new TypeError('Illegal invocation');
            return d.transform.writable;
        }
    }
    Object.defineProperty(TextDecoderStream.prototype, Symbol.toStringTag, {
        value: 'TextDecoderStream', configurable: true,
    });
    globalThis.TextDecoderStream = TextDecoderStream;
})();
