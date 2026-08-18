// node:crypto — purpose-written subset over the sandbox crypto ops
// (OS CSPRNG + RustCrypto digests; server/src/engine/crypto.rs). Sized for
// what npm packages actually import on the client path: uuid needs
// randomFillSync/randomUUID/createHash, content hashing needs
// createHash/createHmac, token comparison needs timingSafeEqual.
//
// Deliberate divergences from Node:
// - Hash/Hmac are one-shot behind the scenes: update() buffers chunks and
//   the digest runs host-side at digest() time. They are plain classes, not
//   Transform streams (no write/end/pipe).
// - Only digest/HMAC/randomness exist. Ciphers, sign/verify, key objects,
//   and KDFs are not exported at all, so `import { createSign } from
//   'node:crypto'` fails at link time and feature detection stays honest.

import { Buffer } from 'node:buffer';

function ops() {
    const bound = globalThis.__mcpV8CryptoOps;
    if (!bound) {
        throw new Error(
            'node:crypto is unavailable: crypto ops were not bound in this runtime');
    }
    return bound;
}

function nodeError(Ctor, code, message) {
    const err = new Ctor(message);
    err.code = code;
    return err;
}

// ── bytes helpers ────────────────────────────────────────────────────────

// Node name (openssl-style) → the wire name the digest op speaks. MD5 is
// not a WebCrypto algorithm and the host ops stay WebCrypto-conformant, so
// it maps to the in-shim implementation below instead.
const HASH_NAMES = {
    'md5': 'MD5',
    'sha1': 'SHA-1', 'sha-1': 'SHA-1',
    'sha256': 'SHA-256', 'sha-256': 'SHA-256',
    'sha384': 'SHA-384', 'sha-384': 'SHA-384',
    'sha512': 'SHA-512', 'sha-512': 'SHA-512',
};

function normalizeHashName(algorithm) {
    if (typeof algorithm !== 'string') {
        throw nodeError(TypeError, 'ERR_INVALID_ARG_TYPE',
            'The "algorithm" argument must be of type string');
    }
    const wire = HASH_NAMES[algorithm.toLowerCase()];
    if (!wire) {
        throw nodeError(Error, 'ERR_CRYPTO_INVALID_DIGEST',
            'Invalid digest: ' + algorithm);
    }
    return wire;
}

// data as accepted by update()/hash(): string (with encoding) or view.
function dataToBytes(data, encoding, method) {
    if (typeof data === 'string') {
        return new Uint8Array(Buffer.from(data, encoding || 'utf8'));
    }
    if (ArrayBuffer.isView(data)) {
        return new Uint8Array(
            data.buffer.slice(data.byteOffset, data.byteOffset + data.byteLength));
    }
    throw nodeError(TypeError, 'ERR_INVALID_ARG_TYPE',
        'The "data" argument passed to ' + method +
        ' must be of type string or an instance of Buffer, TypedArray, or DataView');
}

function concatBytes(chunks) {
    let total = 0;
    for (const chunk of chunks) total += chunk.length;
    const out = new Uint8Array(total);
    let offset = 0;
    for (const chunk of chunks) {
        out.set(chunk, offset);
        offset += chunk.length;
    }
    return out;
}

function encodeDigest(bytes, encoding) {
    const buf = Buffer.from(bytes);
    if (encoding === undefined || encoding === 'buffer') return buf;
    return buf.toString(encoding);
}

// ── MD5 (RFC 1321) ───────────────────────────────────────────────────────
// Kept in JS because MD5 is node-only surface (uuid v3, etag-style content
// hashing): the digest ops speak WebCrypto algorithm names, and teaching
// them MD5 would put a non-WebCrypto algorithm one string away from
// SubtleCrypto. Test-vector-locked in server/tests/node_crypto.rs.

const MD5_K = new Uint32Array([
    0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee,
    0xf57c0faf, 0x4787c62a, 0xa8304613, 0xfd469501,
    0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be,
    0x6b901122, 0xfd987193, 0xa679438e, 0x49b40821,
    0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa,
    0xd62f105d, 0x02441453, 0xd8a1e681, 0xe7d3fbc8,
    0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed,
    0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a,
    0xfffa3942, 0x8771f681, 0x6d9d6122, 0xfde5380c,
    0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70,
    0x289b7ec6, 0xeaa127fa, 0xd4ef3085, 0x04881d05,
    0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665,
    0xf4292244, 0x432aff97, 0xab9423a7, 0xfc93a039,
    0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
    0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1,
    0xf7537e82, 0xbd3af235, 0x2ad7d2bb, 0xeb86d391,
]);
const MD5_S = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22,
    5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20,
    4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23,
    6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
];

function md5Bytes(input) {
    // Pad to 56 mod 64, then a 64-bit little-endian bit length.
    const padded = new Uint8Array((((input.length + 8) >> 6) << 6) + 64);
    padded.set(input);
    padded[input.length] = 0x80;
    const view = new DataView(padded.buffer);
    const bitLen = input.length * 8;
    view.setUint32(padded.length - 8, bitLen >>> 0, true);
    view.setUint32(padded.length - 4, Math.floor(bitLen / 0x100000000), true);

    let a0 = 0x67452301, b0 = 0xefcdab89, c0 = 0x98badcfe, d0 = 0x10325476;
    const m = new Uint32Array(16);
    for (let off = 0; off < padded.length; off += 64) {
        for (let i = 0; i < 16; i++) m[i] = view.getUint32(off + i * 4, true);
        let a = a0, b = b0, c = c0, d = d0;
        for (let i = 0; i < 64; i++) {
            let f, g;
            if (i < 16) { f = (b & c) | (~b & d); g = i; }
            else if (i < 32) { f = (d & b) | (~d & c); g = (5 * i + 1) % 16; }
            else if (i < 48) { f = b ^ c ^ d; g = (3 * i + 5) % 16; }
            else { f = c ^ (b | ~d); g = (7 * i) % 16; }
            const rot = (a + f + MD5_K[i] + m[g]) >>> 0;
            const s = MD5_S[i];
            const next = (b + ((rot << s) | (rot >>> (32 - s)))) >>> 0;
            a = d; d = c; c = b; b = next;
        }
        a0 = (a0 + a) >>> 0; b0 = (b0 + b) >>> 0;
        c0 = (c0 + c) >>> 0; d0 = (d0 + d) >>> 0;
    }

    const out = new Uint8Array(16);
    const ov = new DataView(out.buffer);
    ov.setUint32(0, a0, true);
    ov.setUint32(4, b0, true);
    ov.setUint32(8, c0, true);
    ov.setUint32(12, d0, true);
    return out;
}

// HMAC-MD5 via the standard construction (block size 64).
function hmacMd5Bytes(key, data) {
    if (key.length > 64) key = md5Bytes(key);
    const inner = new Uint8Array(64 + data.length);
    const outer = new Uint8Array(64 + 16);
    for (let i = 0; i < 64; i++) {
        const k = i < key.length ? key[i] : 0;
        inner[i] = k ^ 0x36;
        outer[i] = k ^ 0x5c;
    }
    inner.set(data, 64);
    outer.set(md5Bytes(inner), 64);
    return md5Bytes(outer);
}

function digestBytes(wireName, data) {
    return wireName === 'MD5' ? md5Bytes(data) : ops().digest(wireName, data);
}

// ── hashing ──────────────────────────────────────────────────────────────

export class Hash {
    #algorithm;
    #chunks;
    #finalized = false;

    constructor(algorithm, _options) {
        this.#algorithm = normalizeHashName(algorithm);
        this.#chunks = [];
    }

    update(data, inputEncoding) {
        if (this.#finalized) {
            throw nodeError(Error, 'ERR_CRYPTO_HASH_FINALIZED', 'Digest already called');
        }
        this.#chunks.push(dataToBytes(data, inputEncoding, 'Hash.update'));
        return this;
    }

    digest(encoding) {
        if (this.#finalized) {
            throw nodeError(Error, 'ERR_CRYPTO_HASH_FINALIZED', 'Digest already called');
        }
        this.#finalized = true;
        return encodeDigest(digestBytes(this.#algorithm, concatBytes(this.#chunks)), encoding);
    }

    copy(_options) {
        if (this.#finalized) {
            throw nodeError(Error, 'ERR_CRYPTO_HASH_FINALIZED', 'Digest already called');
        }
        const clone = new Hash(this.#algorithm);
        clone.#chunks = this.#chunks.slice();
        return clone;
    }
}

export class Hmac {
    #algorithm;
    #key;
    #chunks;
    #finalized = false;

    constructor(algorithm, key, _options) {
        this.#algorithm = normalizeHashName(algorithm);
        if (typeof key !== 'string' && !ArrayBuffer.isView(key)) {
            throw nodeError(TypeError, 'ERR_INVALID_ARG_TYPE',
                'The "key" argument must be of type string or an instance of ' +
                'Buffer, TypedArray, or DataView');
        }
        this.#key = dataToBytes(key, 'utf8', 'createHmac');
        this.#chunks = [];
    }

    update(data, inputEncoding) {
        if (this.#finalized) {
            throw nodeError(Error, 'ERR_CRYPTO_HASH_FINALIZED', 'Digest already called');
        }
        this.#chunks.push(dataToBytes(data, inputEncoding, 'Hmac.update'));
        return this;
    }

    digest(encoding) {
        if (this.#finalized) {
            throw nodeError(Error, 'ERR_CRYPTO_HASH_FINALIZED', 'Digest already called');
        }
        this.#finalized = true;
        const data = concatBytes(this.#chunks);
        const mac = this.#algorithm === 'MD5'
            ? hmacMd5Bytes(this.#key, data)
            : ops().hmacSign(this.#algorithm, this.#key, data);
        return encodeDigest(mac, encoding);
    }
}

export function createHash(algorithm, options) {
    return new Hash(algorithm, options);
}

export function createHmac(algorithm, key, options) {
    return new Hmac(algorithm, key, options);
}

// One-shot convenience (Node ≥21.7); what new code reaches for first.
export function hash(algorithm, data, outputEncoding) {
    const bytes = dataToBytes(data, 'utf8', 'crypto.hash');
    return encodeDigest(
        digestBytes(normalizeHashName(algorithm), bytes),
        outputEncoding === undefined ? 'hex' : outputEncoding);
}

export function getHashes() {
    return ['md5', 'sha1', 'sha256', 'sha384', 'sha512'];
}

// ── randomness ───────────────────────────────────────────────────────────

// Fill `view`'s bytes from the CSPRNG. The op fills a plain Uint8Array
// which is then copied in, so Buffer subclasses and DataViews work too.
function fillBytes(view, byteOffset, byteLength) {
    const bytes = new Uint8Array(byteLength);
    ops().getRandomValues(bytes);
    new Uint8Array(view.buffer, view.byteOffset + byteOffset, byteLength).set(bytes);
}

function checkRange(name, value, min, max) {
    if (typeof value !== 'number' || !Number.isInteger(value) ||
        value < min || value > max) {
        throw nodeError(RangeError, 'ERR_OUT_OF_RANGE',
            'The value of "' + name + '" is out of range. It must be an integer ' +
            '>= ' + min + ' && <= ' + max + '. Received ' + value);
    }
}

export function randomBytes(size, callback) {
    checkRange('size', size, 0, 0x7fffffff);
    const buf = Buffer.alloc(size);
    fillBytes(buf, 0, size);
    if (typeof callback === 'function') {
        queueMicrotask(() => callback(null, buf));
        return;
    }
    return buf;
}

export function randomFillSync(buffer, offset, size) {
    if (!ArrayBuffer.isView(buffer)) {
        throw nodeError(TypeError, 'ERR_INVALID_ARG_TYPE',
            'The "buffer" argument must be an instance of Buffer, TypedArray, or DataView');
    }
    if (offset === undefined) offset = 0;
    checkRange('offset', offset, 0, buffer.byteLength);
    if (size === undefined) size = buffer.byteLength - offset;
    checkRange('size', size, 0, buffer.byteLength - offset);
    fillBytes(buffer, offset, size);
    return buffer;
}

export function randomFill(buffer, offset, size, callback) {
    if (typeof offset === 'function') {
        callback = offset; offset = undefined; size = undefined;
    } else if (typeof size === 'function') {
        callback = size; size = undefined;
    }
    if (typeof callback !== 'function') {
        throw nodeError(TypeError, 'ERR_INVALID_ARG_TYPE',
            'The "callback" argument must be of type function');
    }
    const result = randomFillSync(buffer, offset, size);
    queueMicrotask(() => callback(null, result));
}

export function randomUUID(options) {
    if (options !== undefined) {
        if (typeof options !== 'object' || options === null) {
            throw nodeError(TypeError, 'ERR_INVALID_ARG_TYPE',
                'The "options" argument must be of type object');
        }
        // Entropy caching is a Node batching optimization; the only valid
        // values are still enforced so feature probes behave.
        if (options.disableEntropyCache !== undefined &&
            typeof options.disableEntropyCache !== 'boolean') {
            throw nodeError(TypeError, 'ERR_INVALID_ARG_TYPE',
                'The "options.disableEntropyCache" property must be of type boolean');
        }
    }
    return ops().randomUUID();
}

const MAX_RANDOM_RANGE = 281474976710655; // 2**48 - 1: fits 6 CSPRNG bytes.

export function randomInt(min, max, callback) {
    if (typeof max === 'function') {
        callback = max; max = min; min = 0;
    } else if (max === undefined) {
        max = min; min = 0;
    }
    if (!Number.isSafeInteger(min) || !Number.isSafeInteger(max)) {
        throw nodeError(TypeError, 'ERR_INVALID_ARG_TYPE',
            'The "min" and "max" arguments must be safe integers');
    }
    const range = max - min;
    if (!(range > 0)) {
        throw nodeError(RangeError, 'ERR_OUT_OF_RANGE',
            'The value of "max" is out of range. It must be greater than the value of "min" (' +
            min + '). Received ' + max);
    }
    if (range > MAX_RANDOM_RANGE) {
        throw nodeError(RangeError, 'ERR_OUT_OF_RANGE',
            'The value of "max - min" is out of range. It must be <= ' +
            MAX_RANDOM_RANGE + '. Received ' + range);
    }
    // Rejection sampling over 48-bit draws to avoid modulo bias.
    const excess = 2 ** 48 % range;
    const limit = 2 ** 48 - excess;
    const bytes = new Uint8Array(6);
    let draw;
    do {
        ops().getRandomValues(bytes);
        draw = bytes[0] * 2 ** 40 + bytes[1] * 2 ** 32 + bytes[2] * 2 ** 24 +
            bytes[3] * 2 ** 16 + bytes[4] * 2 ** 8 + bytes[5];
    } while (draw >= limit);
    const value = min + (draw % range);
    if (typeof callback === 'function') {
        queueMicrotask(() => callback(null, value));
        return;
    }
    return value;
}

// ── comparison ───────────────────────────────────────────────────────────

export function timingSafeEqual(a, b) {
    if (!ArrayBuffer.isView(a) || !ArrayBuffer.isView(b)) {
        throw nodeError(TypeError, 'ERR_INVALID_ARG_TYPE',
            'The "buf1" and "buf2" arguments must be instances of Buffer, TypedArray, or DataView');
    }
    if (a.byteLength !== b.byteLength) {
        throw nodeError(RangeError, 'ERR_CRYPTO_TIMING_SAFE_EQUAL_LENGTH',
            'Input buffers must have the same byte length');
    }
    const av = new Uint8Array(a.buffer, a.byteOffset, a.byteLength);
    const bv = new Uint8Array(b.buffer, b.byteOffset, b.byteLength);
    let diff = 0;
    for (let i = 0; i < av.length; i++) diff |= av[i] ^ bv[i];
    return diff === 0;
}

// ── Web Crypto aliases ───────────────────────────────────────────────────

export const webcrypto = globalThis.crypto;
export const subtle = globalThis.crypto.subtle;

export function getRandomValues(typedArray) {
    return globalThis.crypto.getRandomValues(typedArray);
}

// OpenSSL engine/padding constants don't apply to this backend; the object
// exists so `crypto.constants` property reads don't throw.
export const constants = Object.freeze({});

export default {
    Hash,
    Hmac,
    createHash,
    createHmac,
    hash,
    getHashes,
    randomBytes,
    randomFillSync,
    randomFill,
    randomUUID,
    randomInt,
    timingSafeEqual,
    webcrypto,
    subtle,
    getRandomValues,
    constants,
};
