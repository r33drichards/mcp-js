// node:zlib — one-shot gzip/deflate (de)compression over the web
// CompressionStream / DecompressionStream already in the runtime. Covers the
// callback API gRPC compression filters use (often via util.promisify).
// Streaming classes (createGzip etc.) are not provided.

import { Buffer } from 'node:buffer';

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

export const constants = Object.freeze({
    Z_NO_FLUSH: 0,
    Z_SYNC_FLUSH: 2,
    Z_FINISH: 4,
    Z_DEFAULT_COMPRESSION: -1,
    Z_BEST_SPEED: 1,
    Z_BEST_COMPRESSION: 9,
});

export default {
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
