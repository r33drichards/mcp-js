/// Tests for `CompressionStream` / `DecompressionStream` and the underlying
/// `ReadableStream` / `WritableStream` shims injected by `engine::compression`.
///
/// All assertions go through console.log: the test executes JS that compresses
/// then decompresses some payload, prints the round-tripped result, and we
/// inspect the captured console output.

use std::sync::Once;
use server::engine::ExecutionConfig;

static INIT: Once = Once::new();

fn ensure_v8() {
    INIT.call_once(|| {
        server::engine::initialize_v8();
    });
}

fn console_tree() -> (sled::Tree, std::path::PathBuf) {
    let tmp = std::env::temp_dir().join(format!(
        "mcp-compression-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let db = sled::open(&tmp).expect("Failed to open sled db");
    let tree = db.open_tree("console").expect("Failed to open tree");
    (tree, tmp)
}

fn read_console(tree: &sled::Tree) -> String {
    let mut buf = Vec::new();
    for entry in tree.iter() {
        if let Ok((_, v)) = entry {
            buf.extend_from_slice(&v);
        }
    }
    String::from_utf8_lossy(&buf).to_string()
}

fn run(code: &str) -> String {
    ensure_v8();
    let heap_bytes = 16 * 1024 * 1024;
    let (tree, tmp) = console_tree();
    let config = ExecutionConfig::new(heap_bytes).console_tree(tree.clone());
    let (result, _oom) = server::engine::execute_stateless(code, config);
    if let Err(e) = &result {
        let _ = std::fs::remove_dir_all(&tmp);
        panic!("execution failed: {}", e);
    }
    let out = read_console(&tree);
    let _ = std::fs::remove_dir_all(&tmp);
    out
}

/// Common JS preamble: helpers for round-tripping bytes through a transform
/// stream and converting Uint8Array <-> hex / ASCII strings. The runtime does
/// not provide `TextEncoder` / `TextDecoder`, so the tests use ASCII helpers.
const PREAMBLE: &str = r#"
function toHex(bytes) {
    let s = '';
    for (let i = 0; i < bytes.length; i++) {
        s += bytes[i].toString(16).padStart(2, '0');
    }
    return s;
}
function encodeAscii(s) {
    const out = new Uint8Array(s.length);
    for (let i = 0; i < s.length; i++) out[i] = s.charCodeAt(i) & 0xff;
    return out;
}
function decodeAscii(bytes) {
    let s = '';
    for (let i = 0; i < bytes.length; i++) s += String.fromCharCode(bytes[i]);
    return s;
}
async function pump(transform, chunks) {
    const writer = transform.writable.getWriter();
    const reader = transform.readable.getReader();
    const collected = [];
    const writePromise = (async () => {
        for (const c of chunks) {
            await writer.write(c);
        }
        await writer.close();
    })();
    while (true) {
        const r = await reader.read();
        if (r.done) break;
        collected.push(r.value);
    }
    await writePromise;
    let total = 0;
    for (const c of collected) total += c.byteLength;
    const out = new Uint8Array(total);
    let off = 0;
    for (const c of collected) { out.set(c, off); off += c.byteLength; }
    return out;
}
"#;

#[test]
fn compression_globals_are_present() {
    let out = run(r#"
        console.log("CompressionStream=" + (typeof CompressionStream));
        console.log("DecompressionStream=" + (typeof DecompressionStream));
        console.log("ReadableStream=" + (typeof ReadableStream));
        console.log("WritableStream=" + (typeof WritableStream));
    "#);
    assert!(out.contains("CompressionStream=function"), "got: {}", out);
    assert!(out.contains("DecompressionStream=function"), "got: {}", out);
    assert!(out.contains("ReadableStream=function"), "got: {}", out);
    assert!(out.contains("WritableStream=function"), "got: {}", out);
}

#[test]
fn gzip_round_trip_chunked() {
    let out = run(&format!(r#"
        {preamble}
        (async () => {{
            const enc = {{ encode: encodeAscii }};
            const dec = {{ decode: decodeAscii }};
            const original = "hello world ".repeat(500);
            const bytes = enc.encode(original);
            // Split into 7 uneven chunks to exercise streaming behavior.
            const sizes = [10, 100, 250, 500, 1000, 2000, bytes.length];
            const chunks = [];
            let pos = 0;
            for (const s of sizes) {{
                if (pos >= bytes.length) break;
                const end = Math.min(pos + s, bytes.length);
                chunks.push(bytes.slice(pos, end));
                pos = end;
            }}
            const compressed = await pump(new CompressionStream('gzip'), chunks);
            console.log("gzip-compressed-len=" + compressed.byteLength);
            console.log("gzip-magic=" + compressed[0].toString(16) + compressed[1].toString(16));
            // Feed compressed bytes back, also chunked, through decompression.
            const dchunks = [];
            for (let i = 0; i < compressed.length; i += 17) {{
                dchunks.push(compressed.slice(i, Math.min(i + 17, compressed.length)));
            }}
            const decompressed = await pump(new DecompressionStream('gzip'), dchunks);
            const decoded = dec.decode(decompressed);
            console.log("match=" + (decoded === original));
            console.log("len=" + decoded.length);
        }})().catch(e => console.log("ERR:" + e.message));
    "#, preamble = PREAMBLE));
    assert!(out.contains("gzip-magic=1f8b"), "missing gzip magic: {}", out);
    assert!(out.contains("match=true"), "round-trip mismatch: {}", out);
    assert!(out.contains("len=6000"), "got: {}", out);
}

#[test]
fn deflate_round_trip() {
    let out = run(&format!(r#"
        {preamble}
        (async () => {{
            const enc = {{ encode: encodeAscii }};
            const dec = {{ decode: decodeAscii }};
            const original = "the quick brown fox ".repeat(50);
            const bytes = enc.encode(original);
            const compressed = await pump(new CompressionStream('deflate'), [bytes]);
            const decompressed = await pump(new DecompressionStream('deflate'), [compressed]);
            console.log("match=" + (dec.decode(decompressed) === original));
        }})().catch(e => console.log("ERR:" + e.message));
    "#, preamble = PREAMBLE));
    assert!(out.contains("match=true"), "got: {}", out);
}

#[test]
fn deflate_raw_round_trip() {
    let out = run(&format!(r#"
        {preamble}
        (async () => {{
            const enc = {{ encode: encodeAscii }};
            const dec = {{ decode: decodeAscii }};
            const original = "compress me ".repeat(40);
            const bytes = enc.encode(original);
            const compressed = await pump(new CompressionStream('deflate-raw'), [bytes]);
            const decompressed = await pump(new DecompressionStream('deflate-raw'), [compressed]);
            console.log("match=" + (dec.decode(decompressed) === original));
        }})().catch(e => console.log("ERR:" + e.message));
    "#, preamble = PREAMBLE));
    assert!(out.contains("match=true"), "got: {}", out);
}

#[test]
fn gzip_decompresses_known_payload() {
    // gzip("hello\n") produced by `printf 'hello\n' | gzip -n`
    let out = run(&format!(r#"
        {preamble}
        (async () => {{
            const dec = {{ decode: decodeAscii }};
            const compressed = new Uint8Array([
                0x1f,0x8b,0x08,0x00,0x00,0x00,0x00,0x00,0x00,0x03,
                0xcb,0x48,0xcd,0xc9,0xc9,0xe7,0x02,0x00,
                0x20,0x30,0x3a,0x36,0x06,0x00,0x00,0x00
            ]);
            const out = await pump(new DecompressionStream('gzip'), [compressed]);
            console.log("decoded=" + JSON.stringify(dec.decode(out)));
        }})().catch(e => console.log("ERR:" + e.message));
    "#, preamble = PREAMBLE));
    assert!(out.contains(r#"decoded="hello\n""#), "got: {}", out);
}

#[test]
fn pipe_through_works() {
    let out = run(&format!(r#"
        {preamble}
        (async () => {{
            const enc = {{ encode: encodeAscii }};
            const dec = {{ decode: decodeAscii }};
            const original = "pipeThrough payload";
            const bytes = enc.encode(original);
            const source = new ReadableStream({{
                start(c) {{ c.enqueue(bytes); c.close(); }},
            }});
            const piped = source.pipeThrough(new CompressionStream('gzip'))
                                .pipeThrough(new DecompressionStream('gzip'));
            const reader = piped.getReader();
            let collected = [];
            while (true) {{
                const r = await reader.read();
                if (r.done) break;
                collected.push(r.value);
            }}
            let total = 0;
            for (const c of collected) total += c.byteLength;
            const buf = new Uint8Array(total);
            let off = 0;
            for (const c of collected) {{ buf.set(c, off); off += c.byteLength; }}
            console.log("pipe-match=" + (dec.decode(buf) === original));
        }})().catch(e => console.log("ERR:" + e.message));
    "#, preamble = PREAMBLE));
    assert!(out.contains("pipe-match=true"), "got: {}", out);
}

#[test]
fn async_iteration_works() {
    let out = run(&format!(r#"
        {preamble}
        (async () => {{
            const enc = {{ encode: encodeAscii }};
            const dec = {{ decode: decodeAscii }};
            const original = "iter payload " .repeat(20);
            const bytes = enc.encode(original);
            const cs = new CompressionStream('gzip');
            const writer = cs.writable.getWriter();
            (async () => {{
                await writer.write(bytes);
                await writer.close();
            }})();
            const collected = [];
            for await (const chunk of cs.readable) {{
                collected.push(chunk);
            }}
            let total = 0;
            for (const c of collected) total += c.byteLength;
            console.log("async-iter-bytes=" + total + " positive=" + (total > 0));

            // Decompress
            const ds = new DecompressionStream('gzip');
            const dwriter = ds.writable.getWriter();
            (async () => {{
                for (const c of collected) await dwriter.write(c);
                await dwriter.close();
            }})();
            let acc = new Uint8Array(0);
            for await (const chunk of ds.readable) {{
                const next = new Uint8Array(acc.length + chunk.byteLength);
                next.set(acc); next.set(chunk, acc.length);
                acc = next;
            }}
            console.log("iter-match=" + (dec.decode(acc) === original));
        }})().catch(e => console.log("ERR:" + e.message));
    "#, preamble = PREAMBLE));
    assert!(out.contains("positive=true"), "got: {}", out);
    assert!(out.contains("iter-match=true"), "got: {}", out);
}

#[test]
fn unknown_format_throws() {
    let out = run(r#"
        try {
            new CompressionStream('lzma');
            console.log("no-throw");
        } catch (e) {
            console.log("threw=" + e.message);
        }
    "#);
    assert!(out.contains("threw="), "got: {}", out);
    assert!(out.contains("lzma"), "got: {}", out);
}

#[test]
fn streaming_produces_output_before_close() {
    // Verifies that the encoder sync-flushes on each write — i.e. some bytes
    // become available on the readable side before the writable is closed.
    // We do NOT call writer.close() here: if we can drain a chunk while the
    // writable is still open, streaming is working.
    let out = run(&format!(r#"
        {preamble}
        (async () => {{
            const enc = {{ encode: encodeAscii }};
            const cs = new CompressionStream('gzip');
            const writer = cs.writable.getWriter();
            const reader = cs.readable.getReader();
            const bytes = enc.encode("x".repeat(10000));
            await writer.write(bytes);
            const r = await reader.read();
            console.log("pre-close-done=" + r.done + " bytes=" + (r.value ? r.value.byteLength : 0));
            // Cleanup: cancel the reader, abort the writer.
            reader.releaseLock();
            await writer.abort('test cleanup');
        }})().catch(e => console.log("ERR:" + e.message));
    "#, preamble = PREAMBLE));
    assert!(out.contains("pre-close-done=false"), "expected chunk before close: {}", out);
    assert!(!out.contains("bytes=0"), "expected non-zero bytes: {}", out);
}
