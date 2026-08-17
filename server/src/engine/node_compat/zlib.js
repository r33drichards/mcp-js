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
    constants,
};
