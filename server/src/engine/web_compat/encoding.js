// TextEncoder / TextDecoder (WHATWG Encoding Standard).
//
// TextEncoder is pure JS (always UTF-8, USVString conversion replaces lone
// surrogates, encodeInto included). TextDecoder delegates to encoding_rs
// ops for the full label table, fatal mode, ignoreBOM, and stateful
// streaming. Replaces the compact UTF-8-only bootstrap versions (detected
// by the missing encodeInto).
(function () {
    'use strict';
    // Install unless a full implementation is already present. The probe
    // also upgrades heaps restored from snapshots that carry the older
    // UTF-8-only bootstrap classes (which ignored the label argument).
    try {
        if (new globalThis.TextDecoder('utf-16le')
                .decode(new Uint8Array([0x68, 0x00])) === 'h' &&
            typeof globalThis.TextEncoder.prototype.encodeInto === 'function') {
            return;
        }
    } catch (_) { /* absent or broken: install below */ }
    var opNormalize = Deno.core.ops.op_encoding_normalize;
    var opDecodeOneshot = Deno.core.ops.op_encoding_decode_oneshot;
    var opNewDecoder = Deno.core.ops.op_encoding_new_decoder;
    var opDecodeStream = Deno.core.ops.op_encoding_decode_stream;
    var opCloseDecoder = Deno.core.ops.op_encoding_close_decoder;

    // ── TextEncoder ─────────────────────────────────────────────────────
    var encoderBrand = new WeakMap();

    // Encode `str` (with lone surrogates replaced by U+FFFD) into `out`
    // starting at 0; stop when out is full. Returns [read units, written bytes].
    function encodeUtf8Into(str, out, cap) {
        var read = 0, written = 0;
        var i = 0;
        while (i < str.length) {
            var code = str.charCodeAt(i);
            var units = 1;
            if (code >= 0xd800 && code <= 0xdbff && i + 1 < str.length) {
                var next = str.charCodeAt(i + 1);
                if (next >= 0xdc00 && next <= 0xdfff) {
                    code = 0x10000 + ((code - 0xd800) << 10) + (next - 0xdc00);
                    units = 2;
                }
            }
            if (code >= 0xd800 && code <= 0xdfff) code = 0xfffd; // lone surrogate
            var need = code < 0x80 ? 1 : code < 0x800 ? 2 : code < 0x10000 ? 3 : 4;
            if (written + need > cap) break;
            if (need === 1) {
                out[written++] = code;
            } else if (need === 2) {
                out[written++] = 0xc0 | (code >> 6);
                out[written++] = 0x80 | (code & 0x3f);
            } else if (need === 3) {
                out[written++] = 0xe0 | (code >> 12);
                out[written++] = 0x80 | ((code >> 6) & 0x3f);
                out[written++] = 0x80 | (code & 0x3f);
            } else {
                out[written++] = 0xf0 | (code >> 18);
                out[written++] = 0x80 | ((code >> 12) & 0x3f);
                out[written++] = 0x80 | ((code >> 6) & 0x3f);
                out[written++] = 0x80 | (code & 0x3f);
            }
            i += units;
            read += units;
        }
        return [read, written];
    }

    class TextEncoder {
        constructor() {
            encoderBrand.set(this, true);
        }
        get encoding() {
            if (!encoderBrand.has(this)) throw new TypeError('Illegal invocation');
            return 'utf-8';
        }
        encode(input) {
            if (!encoderBrand.has(this)) throw new TypeError('Illegal invocation');
            var str = input === undefined ? '' : String(input);
            var out = new Uint8Array(str.length * 3);
            var rw = encodeUtf8Into(str, out, out.length);
            // A single code point can need 4 bytes but occupies 2 units, so
            // 3*length always suffices; astral pairs need 4 <= 6.
            return out.slice(0, rw[1]);
        }
        encodeInto(source, destination) {
            if (!encoderBrand.has(this)) throw new TypeError('Illegal invocation');
            if (arguments.length < 2) {
                throw new TypeError(
                    "Failed to execute 'encodeInto': 2 arguments required, but only " +
                    arguments.length + ' present.');
            }
            if (!(destination instanceof Uint8Array)) {
                throw new TypeError(
                    "Failed to execute 'encodeInto': parameter 2 is not of type 'Uint8Array'.");
            }
            var str = String(source);
            var rw = encodeUtf8Into(str, destination, destination.length);
            return { read: rw[0], written: rw[1] };
        }
    }
    Object.defineProperty(TextEncoder.prototype, Symbol.toStringTag, {
        value: 'TextEncoder', configurable: true,
    });
    globalThis.TextEncoder = TextEncoder;

    // ── TextDecoder ─────────────────────────────────────────────────────
    var decoderData = new WeakMap();
    var registry = typeof FinalizationRegistry === 'function'
        ? new FinalizationRegistry(function (rid) {
            try { opCloseDecoder(rid); } catch (_) { /* isolate teardown */ }
        })
        : null;

    function ddata(dec) {
        var d = decoderData.get(dec);
        if (!d) throw new TypeError('Illegal invocation');
        return d;
    }

    function toBytes(input, method) {
        if (input === undefined) return new Uint8Array(0);
        // Detached buffers count as empty per WebIDL get-a-copy semantics.
        try {
            if (input instanceof ArrayBuffer ||
                (typeof SharedArrayBuffer === 'function' && input instanceof SharedArrayBuffer)) {
                return new Uint8Array(input.slice(0));
            }
            if (ArrayBuffer.isView(input)) {
                return new Uint8Array(
                    input.buffer.slice(input.byteOffset, input.byteOffset + input.byteLength));
            }
        } catch (_) {
            return new Uint8Array(0);
        }
        throw new TypeError(
            "Failed to execute '" + method + "': parameter 1 is not of type 'BufferSource'.");
    }

    class TextDecoder {
        constructor(label, options) {
            label = label === undefined ? 'utf-8' : String(label);
            var encoding;
            try {
                encoding = opNormalize(label);
            } catch (e) {
                throw new RangeError(
                    "Failed to construct 'TextDecoder': The encoding label provided ('" +
                    label + "') is invalid.");
            }
            var fatal = false, ignoreBOM = false;
            if (options !== undefined && options !== null) {
                if (typeof options !== 'object' && typeof options !== 'function') {
                    throw new TypeError(
                        "Failed to construct 'TextDecoder': The provided value is not of type 'TextDecoderOptions'.");
                }
                fatal = !!options.fatal;
                ignoreBOM = !!options.ignoreBOM;
            }
            decoderData.set(this, {
                encoding: encoding, fatal: fatal, ignoreBOM: ignoreBOM, rid: null,
            });
        }
        get encoding() { return ddata(this).encoding; }
        get fatal() { return ddata(this).fatal; }
        get ignoreBOM() { return ddata(this).ignoreBOM; }
        decode(input, options) {
            var d = ddata(this);
            var stream = false;
            if (options !== undefined && options !== null) {
                if (typeof options !== 'object' && typeof options !== 'function') {
                    throw new TypeError(
                        "Failed to execute 'decode': The provided value is not of type 'TextDecodeOptions'.");
                }
                stream = !!options.stream;
            }
            var bytes = toBytes(input, 'decode');
            try {
                if (d.rid === null && !stream) {
                    return opDecodeOneshot(d.encoding, bytes, d.fatal, d.ignoreBOM);
                }
                if (d.rid === null) {
                    d.rid = opNewDecoder(d.encoding, d.ignoreBOM);
                    if (registry) registry.register(this, d.rid);
                }
                return opDecodeStream(d.rid, bytes, d.fatal, !stream);
            } catch (e) {
                throw new TypeError('The encoded data was not valid ' +
                    (d.encoding ? 'for encoding ' + d.encoding : '') +
                    (e && e.message ? ': ' + e.message : ''));
            }
        }
    }
    Object.defineProperty(TextDecoder.prototype, Symbol.toStringTag, {
        value: 'TextDecoder', configurable: true,
    });
    globalThis.TextDecoder = TextDecoder;
})();
