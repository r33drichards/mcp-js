// Web Crypto: crypto / Crypto / SubtleCrypto / CryptoKey.
//
// getRandomValues + randomUUID + subtle.digest and HMAC importKey/sign/
// verify, backed by OS CSPRNG and RustCrypto ops. The rest of SubtleCrypto
// (asymmetric keys, AES, derivation) is intentionally absent for now: the
// methods exist but reject with NotSupportedError so feature detection
// behaves predictably.
(function () {
    'use strict';
    if (typeof globalThis.crypto === 'object' && globalThis.crypto !== null &&
        typeof globalThis.SubtleCrypto === 'function') {
        return;
    }
    var opGetRandomValues = Deno.core.ops.op_crypto_get_random_values;
    var opRandomUuid = Deno.core.ops.op_crypto_random_uuid;
    var opDigest = Deno.core.ops.op_crypto_digest;
    var opHmacSign = Deno.core.ops.op_crypto_hmac_sign;

    function notSupported(what) {
        return Promise.reject(new DOMException(what + ' is not supported', 'NotSupportedError'));
    }

    function toBytes(data, method) {
        if (data instanceof ArrayBuffer) return new Uint8Array(data.slice(0));
        if (ArrayBuffer.isView(data)) {
            return new Uint8Array(
                data.buffer.slice(data.byteOffset, data.byteOffset + data.byteLength));
        }
        throw new TypeError(
            "Failed to execute '" + method + "': parameter is not of type 'BufferSource'.");
    }

    function normalizeHash(algorithm, method) {
        var name = typeof algorithm === 'string' ? algorithm :
            (algorithm && algorithm.name);
        if (typeof name !== 'string') {
            throw new TypeError(
                "Failed to execute '" + method + "': Algorithm: name property missing or not a string.");
        }
        var upper = name.toUpperCase();
        if (upper === 'SHA-1' || upper === 'SHA-256' || upper === 'SHA-384' ||
            upper === 'SHA-512') {
            return upper;
        }
        throw new DOMException('Unrecognized algorithm name: ' + name, 'NotSupportedError');
    }

    // ── CryptoKey ───────────────────────────────────────────────────────
    var keyData = new WeakMap();
    var allowKey = false;

    class CryptoKey {
        constructor() {
            if (!allowKey) throw new TypeError('Illegal constructor');
        }
        get type() { return keyData.get(this).type; }
        get extractable() { return keyData.get(this).extractable; }
        get algorithm() { return keyData.get(this).algorithm; }
        get usages() { return keyData.get(this).usages.slice(); }
    }
    Object.defineProperty(CryptoKey.prototype, Symbol.toStringTag, {
        value: 'CryptoKey', configurable: true,
    });
    globalThis.CryptoKey = CryptoKey;

    function makeKey(type, extractable, algorithm, usages, secret) {
        allowKey = true;
        var key;
        try {
            key = new CryptoKey();
        } finally {
            allowKey = false;
        }
        keyData.set(key, {
            type: type, extractable: extractable,
            algorithm: algorithm, usages: usages, secret: secret,
        });
        return key;
    }

    // ── SubtleCrypto ────────────────────────────────────────────────────
    var subtleBrand = new WeakMap();
    var allowSubtle = false;

    class SubtleCrypto {
        constructor() {
            if (!allowSubtle) throw new TypeError('Illegal constructor');
            subtleBrand.set(this, true);
        }
        digest(algorithm, data) {
            try {
                if (!subtleBrand.has(this)) throw new TypeError('Illegal invocation');
                if (arguments.length < 2) {
                    throw new TypeError(
                        "Failed to execute 'digest': 2 arguments required, but only " +
                        arguments.length + ' present.');
                }
                var hash = normalizeHash(algorithm, 'digest');
                var bytes = toBytes(data, 'digest');
                var out = opDigest(hash, bytes);
                return Promise.resolve(
                    out.buffer.slice(out.byteOffset, out.byteOffset + out.byteLength));
            } catch (e) {
                return Promise.reject(e);
            }
        }
        importKey(format, keyMaterial, algorithm, extractable, keyUsages) {
            try {
                if (!subtleBrand.has(this)) throw new TypeError('Illegal invocation');
                var algName = typeof algorithm === 'string' ? algorithm :
                    (algorithm && algorithm.name);
                if (typeof algName !== 'string' || algName.toUpperCase() !== 'HMAC') {
                    return notSupported('importKey for this algorithm');
                }
                if (format !== 'raw') {
                    return notSupported("importKey format '" + format + "'");
                }
                var hash = normalizeHash(algorithm.hash, 'importKey');
                var usages = Array.from(keyUsages || []);
                for (var i = 0; i < usages.length; i++) {
                    if (usages[i] !== 'sign' && usages[i] !== 'verify') {
                        throw new DOMException(
                            'Cannot create a key using the specified key usages.', 'SyntaxError');
                    }
                }
                var secret = toBytes(keyMaterial, 'importKey');
                var alg = {
                    name: 'HMAC',
                    hash: { name: hash },
                    length: secret.length * 8,
                };
                return Promise.resolve(makeKey('secret', !!extractable, alg, usages, secret));
            } catch (e) {
                return Promise.reject(e);
            }
        }
        sign(algorithm, key, data) {
            try {
                if (!subtleBrand.has(this)) throw new TypeError('Illegal invocation');
                var d = keyData.get(key);
                if (!d) throw new TypeError("parameter 2 is not of type 'CryptoKey'.");
                if (d.usages.indexOf('sign') === -1) {
                    throw new DOMException(
                        'Key does not support the sign operation.', 'InvalidAccessError');
                }
                var out = opHmacSign(d.algorithm.hash.name, d.secret, toBytes(data, 'sign'));
                return Promise.resolve(
                    out.buffer.slice(out.byteOffset, out.byteOffset + out.byteLength));
            } catch (e) {
                return Promise.reject(e);
            }
        }
        verify(algorithm, key, signature, data) {
            var self_ = this;
            var sig;
            try {
                sig = toBytes(signature, 'verify');
            } catch (e) {
                return Promise.reject(e);
            }
            return this.sign(algorithm, key, data).then(function (expected) {
                var exp = new Uint8Array(expected);
                if (exp.length !== sig.length) return false;
                var diff = 0;
                for (var i = 0; i < exp.length; i++) diff |= exp[i] ^ sig[i];
                return diff === 0;
            });
        }
        generateKey() { return notSupported('generateKey'); }
        deriveKey() { return notSupported('deriveKey'); }
        deriveBits() { return notSupported('deriveBits'); }
        encrypt() { return notSupported('encrypt'); }
        decrypt() { return notSupported('decrypt'); }
        exportKey(format, key) {
            try {
                if (!subtleBrand.has(this)) throw new TypeError('Illegal invocation');
                var d = keyData.get(key);
                if (!d) throw new TypeError("parameter 2 is not of type 'CryptoKey'.");
                if (!d.extractable) {
                    throw new DOMException('Key is not extractable.', 'InvalidAccessError');
                }
                if (format !== 'raw') return notSupported("exportKey format '" + format + "'");
                var copy = new Uint8Array(d.secret);
                return Promise.resolve(copy.buffer);
            } catch (e) {
                return Promise.reject(e);
            }
        }
        wrapKey() { return notSupported('wrapKey'); }
        unwrapKey() { return notSupported('unwrapKey'); }
    }
    Object.defineProperty(SubtleCrypto.prototype, Symbol.toStringTag, {
        value: 'SubtleCrypto', configurable: true,
    });
    globalThis.SubtleCrypto = SubtleCrypto;

    // ── Crypto ──────────────────────────────────────────────────────────
    var cryptoBrand = new WeakMap();
    var allowCrypto = false;
    var INT_ARRAYS = [
        Int8Array, Uint8Array, Uint8ClampedArray, Int16Array, Uint16Array,
        Int32Array, Uint32Array, BigInt64Array, BigUint64Array,
    ];

    class Crypto {
        constructor() {
            if (!allowCrypto) throw new TypeError('Illegal constructor');
            cryptoBrand.set(this, true);
        }
        get subtle() {
            if (!cryptoBrand.has(this)) throw new TypeError('Illegal invocation');
            var d = cryptoBrand.get(this);
            if (d === true) {
                allowSubtle = true;
                try {
                    d = new SubtleCrypto();
                } finally {
                    allowSubtle = false;
                }
                cryptoBrand.set(this, d);
            }
            return d;
        }
        getRandomValues(array) {
            if (!cryptoBrand.has(this)) throw new TypeError('Illegal invocation');
            if (arguments.length < 1) {
                throw new TypeError(
                    "Failed to execute 'getRandomValues': 1 argument required, but only 0 present.");
            }
            var ok = false;
            for (var i = 0; i < INT_ARRAYS.length; i++) {
                if (array instanceof INT_ARRAYS[i]) { ok = true; break; }
            }
            if (!ok) {
                throw new DOMException(
                    "Failed to execute 'getRandomValues': The provided ArrayBufferView is not an integer array type.",
                    'TypeMismatchError');
            }
            if (array.byteLength > 65536) {
                throw new DOMException(
                    "Failed to execute 'getRandomValues': The requested length exceeds 65536 bytes.",
                    'QuotaExceededError');
            }
            var bytes = new Uint8Array(array.byteLength);
            opGetRandomValues(bytes);
            new Uint8Array(array.buffer, array.byteOffset, array.byteLength).set(bytes);
            return array;
        }
        randomUUID() {
            if (!cryptoBrand.has(this)) throw new TypeError('Illegal invocation');
            return opRandomUuid();
        }
    }
    Object.defineProperty(Crypto.prototype, Symbol.toStringTag, {
        value: 'Crypto', configurable: true,
    });
    globalThis.Crypto = Crypto;
    allowCrypto = true;
    try {
        globalThis.crypto = new Crypto();
    } finally {
        allowCrypto = false;
    }
})();
