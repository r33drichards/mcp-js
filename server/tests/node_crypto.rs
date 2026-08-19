//! Integration tests for the `node:crypto` compat module.
//!
//! Locks in the subset that lets npm packages load and run — uuid@11's
//! exact imports (randomFillSync / createHash / randomUUID), digest and
//! HMAC vectors against the RustCrypto-backed ops, randomness helpers, and
//! timingSafeEqual — while `SubtleCrypto` stays WebCrypto-conformant (MD5
//! exists only on the node surface). JS validates itself and throws on any
//! mismatch; tests assert the execution finished without error.

use std::sync::{Arc, Once};

use server::engine::execution::ExecutionRegistry;
use server::engine::{Engine, initialize_v8};

static INIT: Once = Once::new();

fn ensure_v8() {
    INIT.call_once(|| {
        initialize_v8();
    });
}

fn create_test_engine() -> Engine {
    let tmp = std::env::temp_dir().join(format!(
        "mcp-node-crypto-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let registry =
        ExecutionRegistry::new(tmp.to_str().unwrap()).expect("Failed to create test registry");
    Engine::new_stateless(64 * 1024 * 1024, 30, 4).with_execution_registry(Arc::new(registry))
}

async fn run_js(engine: &Engine, code: &str) -> Result<String, String> {
    let exec_id = engine
        .run_js(code.to_string())
        .execute()
        .await
        .map_err(|error| format!("submit should succeed: {error}"))?;

    for _ in 0..600 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        if let Ok(info) = engine.get_execution(&exec_id) {
            match info.status.as_str() {
                "completed" => return Ok(info.result.unwrap_or_default()),
                "failed" => return Err(info.error.unwrap_or_default()),
                "timed_out" => return Err("execution timed out".to_string()),
                _ => continue,
            }
        }
    }

    Err("timeout waiting for execution".to_string())
}

async fn expect_ok(code: &str) {
    ensure_v8();
    let engine = create_test_engine();
    if let Err(error) = run_js(&engine, code).await {
        panic!("execution failed: {error}");
    }
}

#[tokio::test]
async fn hash_vectors_and_encodings() {
    expect_ok(
        r#"
        import { createHash, hash } from 'node:crypto';
        import { Buffer } from 'node:buffer';

        const vectors = {
            md5: '900150983cd24fb0d6963f7d28e17f72',
            sha1: 'a9993e364706816aba3e25717850c26c9cd0d89d',
            sha256: 'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad',
            sha384: 'cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed8086072ba1e7cc2358baeca134c825a7',
            sha512: 'ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f',
        };
        for (const [alg, want] of Object.entries(vectors)) {
            const got = createHash(alg).update('abc').digest('hex');
            if (got !== want) throw new Error(alg + ': ' + got);
        }

        // Chained updates and non-utf8 input encodings hash the same bytes.
        if (createHash('sha256').update('a').update('bc').digest('hex') !== vectors.sha256) {
            throw new Error('chained update mismatch');
        }
        if (createHash('sha256').update('616263', 'hex').digest('hex') !== vectors.sha256) {
            throw new Error('hex input encoding mismatch');
        }
        if (createHash('sha256').update(Buffer.from('abc')).digest('hex') !== vectors.sha256) {
            throw new Error('Buffer input mismatch');
        }

        // Output encodings: default Buffer, base64, and the one-shot helper.
        const buf = createHash('sha256').update('abc').digest();
        if (!Buffer.isBuffer(buf) || buf.length !== 32) throw new Error('digest() should be a 32-byte Buffer');
        const b64 = createHash('sha256').update('abc').digest('base64');
        if (b64 !== 'ungWv48Bz+pBQUDeXa4iI7ADYaOWF3qctBD/YfIAFa0=') throw new Error('base64: ' + b64);
        if (hash('sha256', 'abc') !== vectors.sha256) throw new Error('one-shot hash mismatch');
        if (!Buffer.isBuffer(hash('sha256', 'abc', 'buffer'))) throw new Error('hash(..., "buffer")');

        // copy() forks the pending state.
        const h = createHash('sha256').update('a');
        const c = h.copy();
        h.update('bc');
        c.update('bc');
        if (h.digest('hex') !== vectors.sha256 || c.digest('hex') !== vectors.sha256) {
            throw new Error('copy() mismatch');
        }

        // MD5 is implemented in the shim (RFC 1321), not the host ops: pin
        // the empty message, a multi-block input, and both sides of the
        // 56-byte padding boundary.
        const md5Vectors = [
            ['', 'd41d8cd98f00b204e9800998ecf8427e'],
            ['The quick brown fox jumps over the lazy dog', '9e107d9d372bb6826bd81d3542a419d6'],
            ['a'.repeat(56), '3b0c8ac703f828b04c6c197006d17218'],
            ['a'.repeat(100), '36a92cc94a9e0fa21f625f8bfb007adf'],
        ];
        for (const [input, want] of md5Vectors) {
            const got = createHash('md5').update(input).digest('hex');
            if (got !== want) throw new Error('md5(' + input.length + ' bytes): ' + got);
        }

        // Finalization and unknown algorithms fail the Node way.
        const done = createHash('sha256').update('abc');
        done.digest();
        try { done.digest(); throw new Error('second digest should throw'); }
        catch (e) { if (e.code !== 'ERR_CRYPTO_HASH_FINALIZED') throw e; }
        try { createHash('sha3-999'); throw new Error('unknown digest should throw'); }
        catch (e) { if (e.code !== 'ERR_CRYPTO_INVALID_DIGEST') throw e; }
        "#,
    )
    .await;
}

#[tokio::test]
async fn hmac_vectors() {
    expect_ok(
        r#"
        import { createHmac } from 'node:crypto';
        import { Buffer } from 'node:buffer';

        const msg = 'The quick brown fox jumps over the lazy dog';
        const sha256 = createHmac('sha256', 'key').update(msg).digest('hex');
        if (sha256 !== 'f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8') {
            throw new Error('hmac-sha256: ' + sha256);
        }
        const md5 = createHmac('md5', 'key').update(msg).digest('hex');
        if (md5 !== '80070713463e7749b90c2dc24911e275') throw new Error('hmac-md5: ' + md5);

        // Keys longer than the 64-byte block are hashed first (RFC 2104).
        const longKey = createHmac('md5', 'k'.repeat(100)).update('data').digest('hex');
        if (longKey !== '38a034384b6cbf08275f34dc859e6d6b') throw new Error('hmac-md5 long key: ' + longKey);

        // Buffer keys sign identically to their string form.
        const viaBuffer = createHmac('sha256', Buffer.from('key')).update(msg).digest('hex');
        if (viaBuffer !== sha256) throw new Error('Buffer key mismatch');

        try { createHmac('sha256', 42); throw new Error('numeric key should throw'); }
        catch (e) { if (e.code !== 'ERR_INVALID_ARG_TYPE') throw e; }
        "#,
    )
    .await;
}

#[tokio::test]
async fn randomness_helpers() {
    expect_ok(
        r#"
        import {
            randomBytes, randomFillSync, randomFill, randomUUID, randomInt,
        } from 'node:crypto';
        import { Buffer } from 'node:buffer';

        const bytes = randomBytes(32);
        if (!Buffer.isBuffer(bytes) || bytes.length !== 32) throw new Error('randomBytes shape');
        if (bytes.every((b) => b === 0)) throw new Error('randomBytes returned zeros');
        if (randomBytes(0).length !== 0) throw new Error('randomBytes(0)');

        // offset/size fill only the requested byte range.
        const buf = Buffer.alloc(16);
        if (randomFillSync(buf, 4, 8) !== buf) throw new Error('randomFillSync should return its buffer');
        for (let i = 0; i < 4; i++) {
            if (buf[i] !== 0 || buf[12 + i] !== 0) throw new Error('randomFillSync wrote outside range');
        }
        if (buf.subarray(4, 12).every((b) => b === 0)) throw new Error('randomFillSync left range zeroed');

        // Plain typed arrays are accepted (the exact uuid@11 rng call).
        const pool = new Uint8Array(256);
        if (randomFillSync(pool) !== pool) throw new Error('randomFillSync(Uint8Array)');
        if (pool.every((b) => b === 0)) throw new Error('pool not filled');

        const uuid = randomUUID();
        if (!/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(uuid)) {
            throw new Error('bad uuid: ' + uuid);
        }

        for (let i = 0; i < 100; i++) {
            const v = randomInt(10);
            if (!Number.isInteger(v) || v < 0 || v >= 10) throw new Error('randomInt(10): ' + v);
            const w = randomInt(5, 7);
            if (w !== 5 && w !== 6) throw new Error('randomInt(5, 7): ' + w);
        }
        try { randomInt(5, 5); throw new Error('empty range should throw'); }
        catch (e) { if (e.code !== 'ERR_OUT_OF_RANGE') throw e; }

        // Callback forms complete asynchronously with (null, result).
        const cbBytes = await new Promise((resolve, reject) => {
            randomBytes(8, (err, b) => err ? reject(err) : resolve(b));
        });
        if (cbBytes.length !== 8) throw new Error('randomBytes callback');
        await new Promise((resolve, reject) => {
            randomFill(Buffer.alloc(8), (err, b) => err ? reject(err) : resolve(b));
        });
        const cbInt = await new Promise((resolve, reject) => {
            randomInt(10, (err, v) => err ? reject(err) : resolve(v));
        });
        if (cbInt < 0 || cbInt >= 10) throw new Error('randomInt callback: ' + cbInt);
        "#,
    )
    .await;
}

#[tokio::test]
async fn timing_safe_equal() {
    expect_ok(
        r#"
        import { timingSafeEqual } from 'node:crypto';
        import { Buffer } from 'node:buffer';

        if (!timingSafeEqual(Buffer.from('secret'), Buffer.from('secret'))) {
            throw new Error('equal buffers should compare true');
        }
        if (timingSafeEqual(Buffer.from('secret'), Buffer.from('secreT'))) {
            throw new Error('different buffers should compare false');
        }
        try {
            timingSafeEqual(Buffer.from('a'), Buffer.from('ab'));
            throw new Error('length mismatch should throw');
        } catch (e) {
            if (e.code !== 'ERR_CRYPTO_TIMING_SAFE_EQUAL_LENGTH') throw e;
        }
        "#,
    )
    .await;
}

#[tokio::test]
async fn webcrypto_aliases_and_bare_specifier() {
    expect_ok(
        r#"
        import crypto from 'crypto';

        if (crypto.webcrypto !== globalThis.crypto) throw new Error('webcrypto alias');
        if (crypto.subtle !== globalThis.crypto.subtle) throw new Error('subtle alias');
        const arr = new Uint8Array(8);
        if (crypto.getRandomValues(arr) !== arr) throw new Error('getRandomValues alias');

        // node digest and SubtleCrypto digest agree on the same input.
        const viaSubtle = new Uint8Array(
            await crypto.subtle.digest('SHA-256', new TextEncoder().encode('abc')));
        const viaNode = crypto.createHash('sha256').update('abc').digest();
        if (viaSubtle.length !== viaNode.length ||
            !viaSubtle.every((b, i) => b === viaNode[i])) {
            throw new Error('subtle/node digest mismatch');
        }

        // MD5 stays node-only: the WebCrypto surface must keep rejecting it.
        let rejected = false;
        try { await crypto.subtle.digest('MD5', new Uint8Array(1)); }
        catch (e) { rejected = e.name === 'NotSupportedError'; }
        if (!rejected) throw new Error('subtle.digest(MD5) should reject');
        "#,
    )
    .await;
}

#[tokio::test]
async fn uuid_v11_import_shape() {
    // The exact named imports uuid@11 links against — this failing is the
    // "Unknown node builtin module: 'crypto'" report, one layer down.
    expect_ok(
        r#"
        import { randomFillSync, createHash, randomUUID } from 'node:crypto';

        const rnds8Pool = new Uint8Array(256);
        randomFillSync(rnds8Pool);

        const digest = createHash('sha1').update(new Uint8Array([1, 2, 3])).digest();
        if (digest.length !== 20) throw new Error('sha1 digest length: ' + digest.length);

        if (typeof randomUUID() !== 'string') throw new Error('randomUUID');
        "#,
    )
    .await;
}
