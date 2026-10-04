// node:zlib — one-shot gzip/deflate (de)compression over the web
// CompressionStream / DecompressionStream already in the runtime. Covers the
// callback API gRPC compression filters use (often via util.promisify).
// Includes synchronous gzip/deflate APIs and the legacy Deflate/Inflate
// stream surface used by pngjs. Advanced tuning and backpressure are partial.

import { Buffer } from 'node:buffer';
import { Stream } from 'node:stream';

async function transformBytes(TransformCtor, format, input) {
    const stream = new TransformCtor(format);
    const writer = stream.writable.getWriter();
    const reader = stream.readable.getReader();
    const chunks = [];
    const readAll = (async () => {
        for (;;) {
            const { done, value } = await reader.read();
            if (done) break;
            chunks.push(value);
        }
    })();
    await writer.write(toUint8(input));
    await writer.close();
    await readAll;
    let total = 0;
    for (const chunk of chunks) total += chunk.length;
    const out = new Uint8Array(total);
    let offset = 0;
    for (const chunk of chunks) {
        out.set(chunk, offset);
        offset += chunk.length;
    }
    return Buffer.from(out.buffer, out.byteOffset, out.byteLength);
}

function toUint8(input) {
    if (input instanceof Uint8Array) return input;
    if (input instanceof ArrayBuffer) return new Uint8Array(input);
    if (ArrayBuffer.isView(input)) {
        return new Uint8Array(input.buffer, input.byteOffset, input.byteLength);
    }
    return new TextEncoder().encode(String(input));
}

const CRC32_TABLE = (() => {
    const table = new Uint32Array(256);
    for (let index = 0; index < table.length; index++) {
        let value = index;
        for (let bit = 0; bit < 8; bit++) {
            value = (value >>> 1) ^ ((value & 1) ? 0xedb88320 : 0);
        }
        table[index] = value >>> 0;
    }
    return table;
})();

function invalidArgType(name, expected, value) {
    const error = new TypeError(
        `The "${name}" argument must be of type ${expected}. Received ${String(value)}`,
    );
    error.code = 'ERR_INVALID_ARG_TYPE';
    return error;
}

export function crc32(data, value = 0) {
    let bytes;
    if (typeof data === 'string') {
        bytes = new TextEncoder().encode(data);
    } else if (ArrayBuffer.isView(data)) {
        bytes = new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
    } else {
        throw invalidArgType('data', 'string or an instance of Buffer, TypedArray, or DataView', data);
    }

    if (value === undefined) value = 0;
    if (typeof value !== 'number') {
        throw invalidArgType('value', 'number', value);
    }
    if (!Number.isInteger(value) || value < 0 || value > 0xffffffff) {
        const error = new RangeError('The value of "value" is out of range');
        error.code = 'ERR_OUT_OF_RANGE';
        throw error;
    }

    let checksum = (value ^ 0xffffffff) >>> 0;
    for (const byte of bytes) {
        checksum = (CRC32_TABLE[(checksum ^ byte) & 0xff] ^ (checksum >>> 8)) >>> 0;
    }
    return (checksum ^ 0xffffffff) >>> 0;
}

function callbackified(format, TransformCtor) {
    return function (input, optionsOrCallback, maybeCallback) {
        const callback =
            typeof optionsOrCallback === 'function' ? optionsOrCallback : maybeCallback;
        if (typeof callback !== 'function') {
            throw new TypeError('zlib: callback is required (sync APIs are not provided)');
        }
        transformBytes(TransformCtor, format, input).then(
            (result) => callback(null, result),
            (err) => callback(err instanceof Error ? err : new Error(String(err))));
    };
}

export const gzip = callbackified('gzip', CompressionStream);
export const gunzip = callbackified('gzip', DecompressionStream);
export const deflate = callbackified('deflate', CompressionStream);
export const inflate = callbackified('deflate', DecompressionStream);
export const deflateRaw = callbackified('deflate-raw', CompressionStream);
export const inflateRaw = callbackified('deflate-raw', DecompressionStream);
export const unzip = gunzip;

// The flate2 resources are synchronous ops, so Node's sync API does not
// have to block on a web-stream Promise. Each call closes its native resource.
const core = Deno.core;
const ops = core.ops;

function compressionOptions(options = {}) {
    const level = options.level ?? -1;
    const chunkSize = options.chunkSize ?? 16 * 1024;
    if (!Number.isInteger(level) || level < -1 || level > 9) {
        throw new RangeError('zlib level must be an integer from -1 to 9');
    }
    if (!Number.isInteger(chunkSize) || chunkSize < 64) {
        throw new RangeError('zlib chunkSize must be an integer of at least 64');
    }
    return { level, chunkSize };
}

function syncBytes(format, decompress, input, options) {
    const { level } = compressionOptions(options);
    const rid = ops.op_node_zlib_new(format, decompress, level);
    try {
        const first = ops.op_compression_write(rid, toUint8(input));
        const last = ops.op_compression_finish(rid);
        const output = Buffer.concat([Buffer.from(first), Buffer.from(last)]);
        if (options?.maxOutputLength !== undefined && output.length > options.maxOutputLength) {
            throw new RangeError('Decompressed output exceeds maxOutputLength');
        }
        return output;
    } finally {
        core.tryClose(rid);
    }
}

export const gzipSync = (input, options) => syncBytes('gzip', false, input, options);
export const gunzipSync = (input, options) => syncBytes('gzip', true, input, options);
export const deflateSync = (input, options) => syncBytes('deflate', false, input, options);
export const inflateSync = (input, options) => syncBytes('deflate', true, input, options);
export const deflateRawSync = (input, options) => syncBytes('deflate-raw', false, input, options);
export const inflateRawSync = (input, options) => syncBytes('deflate-raw', true, input, options);

// Function-style constructors preserve legacy subclasses such as pngjs's
// bounded Inflate adapter, which invokes zlib.Inflate.call(this, options).
function initZlib(self, format, decompress, options) {
    Stream.call(self);
    const { level, chunkSize } = compressionOptions(options);
    self._chunkSize = chunkSize;
    self._outOffset = 0;
    self._outBuffer = Buffer.allocUnsafe(chunkSize);
    self._finishFlushFlag = options?.finishFlush ?? 4;
    self._hadError = false;
    self._writeState = new Uint32Array(2);
    self.readable = true;
    self.writable = true;
    self._rid = ops.op_node_zlib_new(format, decompress, level);
    let pending = new Uint8Array(0);
    self._handle = {
        writeSync(flush, input, inOffset, inLength, output, outOffset, outLength) {
            try {
                let consumed = 0;
                if (pending.length === 0 && self._rid !== null) {
                    pending = ops.op_compression_write(self._rid,
                        toUint8(input).subarray(inOffset, inOffset + inLength));
                    consumed = inLength;
                    if (flush === 4) {
                        const rid = self._rid;
                        self._rid = null;
                        pending = Buffer.concat([Buffer.from(pending),
                            Buffer.from(ops.op_compression_finish(rid))]);
                    }
                }
                const count = Math.min(outLength, pending.length);
                output.set(pending.subarray(0, count), outOffset);
                pending = pending.subarray(count);
                self._writeState[0] = outLength - count;
                self._writeState[1] = inLength - consumed;
                // pngjs supports both the pre-Node-9 return value and the
                // modern _writeState layout (output remaining, input remaining).
                return [inLength - consumed, outLength - count];
            } catch (error) {
                self._hadError = true;
                self.close();
                self.emit('error', error);
                return [0, outLength];
            }
        },
        close() {
            if (self._rid !== null) core.tryClose(self._rid);
            self._rid = null;
            pending = new Uint8Array(0);
        },
    };
}

function zlibConstructor(format, decompress) {
    function Zlib(options) {
        if (!(this instanceof Zlib)) return new Zlib(options);
        initZlib(this, format, decompress, options);
    }
    Object.setPrototypeOf(Zlib.prototype, Stream.prototype);
    Object.setPrototypeOf(Zlib, Stream);
    Zlib.prototype.write = function (input, encoding, callback) {
        if (typeof encoding === 'function') callback = encoding;
        if (!this.writable || this._rid === null) throw new Error('zlib stream is closed');
        try {
            const output = ops.op_compression_write(this._rid, toUint8(input));
            if (output.length) this.emit('data', Buffer.from(output));
            if (callback) Promise.resolve().then(() => callback(null));
        } catch (error) {
            this.destroy(error);
            if (callback) Promise.resolve().then(() => callback(error));
        }
        return true;
    };
    Zlib.prototype.end = function (input, encoding, callback) {
        if (typeof input === 'function') { callback = input; input = undefined; }
        if (typeof encoding === 'function') callback = encoding;
        if (input !== undefined && input !== null) this.write(input, encoding);
        this.writable = false;
        if (this._rid === null) return this;
        try {
            const rid = this._rid;
            this._rid = null;
            const output = ops.op_compression_finish(rid);
            if (output.length) this.emit('data', Buffer.from(output));
            Promise.resolve().then(() => {
                this.readable = false;
                this.emit('finish');
                this.emit('end');
                this.emit('close');
                if (callback) callback();
            });
        } catch (error) {
            this.destroy(error);
        }
        return this;
    };
    Zlib.prototype.close = function (callback) {
        if (this._handle) this._handle.close();
        this._handle = null;
        if (callback) Promise.resolve().then(callback);
        return this;
    };
    Zlib.prototype.destroy = function (error) {
        this.close();
        this.writable = this.readable = false;
        Promise.resolve().then(() => {
            if (error) this.emit('error', error);
            this.emit('close');
        });
        return this;
    };
    return Zlib;
}

export const Deflate = zlibConstructor('deflate', false);
export const Inflate = zlibConstructor('deflate', true);
export const createDeflate = (options) => new Deflate(options);
export const createInflate = (options) => new Inflate(options);
export const Z_MIN_CHUNK = 64;
export const Z_FINISH = 4;

export const constants = Object.freeze({
    Z_MIN_CHUNK,
    Z_NO_FLUSH: 0,
    Z_SYNC_FLUSH: 2,
    Z_FINISH: 4,
    Z_DEFAULT_COMPRESSION: -1,
    Z_BEST_SPEED: 1,
    Z_BEST_COMPRESSION: 9,
});

export default {
    Deflate, Inflate, createDeflate, createInflate, Z_MIN_CHUNK, Z_FINISH,
    gzipSync, gunzipSync, deflateSync, inflateSync, deflateRawSync, inflateRawSync,
    gzip,
    gunzip,
    deflate,
    inflate,
    deflateRaw,
    inflateRaw,
    unzip,
    crc32,
    constants,
};
