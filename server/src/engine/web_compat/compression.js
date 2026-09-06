// CompressionStream / DecompressionStream (Compression Standard), built on
// TransformStream with flate2 ops doing the streaming byte work.
(function () {
    'use strict';
    if (typeof globalThis.CompressionStream === 'function' &&
        typeof globalThis.DecompressionStream === 'function') {
        return;
    }
    var opNew = Deno.core.ops.op_compression_new;
    var opWrite = Deno.core.ops.op_compression_write;
    var opFinish = Deno.core.ops.op_compression_finish;

    function toBytes(chunk, what) {
        if (typeof SharedArrayBuffer === 'function' &&
            (chunk instanceof SharedArrayBuffer ||
             (ArrayBuffer.isView(chunk) && chunk.buffer instanceof SharedArrayBuffer))) {
            throw new TypeError(
                "Failed to execute 'transform' on '" + what + "': shared buffers are not allowed.");
        }
        if (chunk instanceof ArrayBuffer) return new Uint8Array(chunk.slice(0));
        if (ArrayBuffer.isView(chunk)) {
            return new Uint8Array(
                chunk.buffer.slice(chunk.byteOffset, chunk.byteOffset + chunk.byteLength));
        }
        throw new TypeError(
            "Failed to execute 'transform' on '" + what + "': chunk is not of type 'BufferSource'.");
    }

    function makeStreamClass(name, decompress) {
        var dataMap = new WeakMap();
        var cls = class {
            constructor(format) {
                if (arguments.length < 1) {
                    throw new TypeError(
                        "Failed to construct '" + name + "': 1 argument required, but only 0 present.");
                }
                format = String(format);
                if (format !== 'gzip' && format !== 'deflate' && format !== 'deflate-raw' &&
                    format !== 'brotli') {
                    throw new TypeError(
                        "Failed to construct '" + name + "': Unsupported compression format: '" +
                        format + "'");
                }
                var rid = opNew(format, decompress);
                var finished = false;
                var junkError = false;
                var transform = new TransformStream({
                    transform: function (chunk, controller) {
                        if (junkError) {
                            throw new TypeError('Junk found after end of compressed data.');
                        }
                        var bytes = toBytes(chunk, name);
                        var out = opWrite(rid, bytes);
                        if (out.length > 0) controller.enqueue(new Uint8Array(out));
                        // Trailing junk: deliver the output above, then error
                        // the stream (readers see the value, then a TypeError).
                        if (Deno.core.ops.op_compression_has_junk(rid)) {
                            junkError = true;
                            throw new TypeError('Junk found after end of compressed data.');
                        }
                    },
                    flush: function (controller) {
                        finished = true;
                        var out = opFinish(rid);
                        if (out.length > 0) controller.enqueue(new Uint8Array(out));
                    },
                });
                dataMap.set(this, { transform: transform, format: format });
            }
            get readable() {
                var d = dataMap.get(this);
                if (!d) throw new TypeError('Illegal invocation');
                return d.transform.readable;
            }
            get writable() {
                var d = dataMap.get(this);
                if (!d) throw new TypeError('Illegal invocation');
                return d.transform.writable;
            }
        };
        Object.defineProperty(cls, 'name', { value: name, configurable: true });
        Object.defineProperty(cls.prototype, Symbol.toStringTag, {
            value: name, configurable: true,
        });
        return cls;
    }

    globalThis.CompressionStream = makeStreamClass('CompressionStream', false);
    globalThis.DecompressionStream = makeStreamClass('DecompressionStream', true);
})();
