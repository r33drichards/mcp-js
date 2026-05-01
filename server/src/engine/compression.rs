//! Streaming compression / decompression for the JavaScript runtime.
//!
//! Provides the Web-standard `CompressionStream` and `DecompressionStream`
//! constructors, backed by `flate2` for the actual gzip/deflate work.
//!
//! # How streaming works
//!
//! Each `CompressionStream` instance allocates a stateful encoder (or decoder)
//! in Rust, identified by a `u32` id stored in the runtime's `OpState`.
//! Three sync ops drive the stream lifecycle:
//!
//! - `op_compression_create(format, decompress) -> id`
//! - `op_compression_write(id, chunk) -> output_chunk`  — sync-flushes the
//!   encoder so each JS `writer.write()` produces output without waiting
//!   for the whole stream to finish.
//! - `op_compression_finish(id) -> tail_chunk` — flushes the final deflate
//!   block(s) and removes the encoder.
//!
//! A small JS shim then exposes `ReadableStream` / `WritableStream` /
//! `CompressionStream` / `DecompressionStream` on `globalThis`, wired so
//! that `writable.write(chunk)` runs the op and `enqueue`s the produced
//! bytes onto the paired readable side. Closing the writable flushes the
//! tail and closes the readable; aborting cancels the encoder.
//!
//! # Supported formats
//!
//! - `"gzip"`         — RFC 1952 (gzip header + raw deflate + CRC32 trailer)
//! - `"deflate"`      — RFC 1950 (zlib wrapper around raw deflate)
//! - `"deflate-raw"`  — RFC 1951 (raw deflate, no wrapper)
//!
//! Format names match the WHATWG Compression Streams spec.

use std::collections::HashMap;
use std::io::Write;

use deno_core::{JsRuntime, OpState, op2};
use deno_error::JsErrorBox;
use flate2::Compression;
use flate2::write::{
    DeflateDecoder, DeflateEncoder, GzDecoder, GzEncoder, ZlibDecoder, ZlibEncoder,
};

// ── Per-runtime registry stored in OpState ──────────────────────────────

enum Stream {
    GzipEnc(GzEncoder<Vec<u8>>),
    ZlibEnc(ZlibEncoder<Vec<u8>>),
    DeflateRawEnc(DeflateEncoder<Vec<u8>>),
    GzipDec(GzDecoder<Vec<u8>>),
    ZlibDec(ZlibDecoder<Vec<u8>>),
    DeflateRawDec(DeflateDecoder<Vec<u8>>),
}

#[derive(Default)]
pub struct CompressionRegistry {
    next_id: u32,
    streams: HashMap<u32, Stream>,
}

fn registry(state: &mut OpState) -> &mut CompressionRegistry {
    if !state.has::<CompressionRegistry>() {
        state.put(CompressionRegistry::default());
    }
    state.borrow_mut::<CompressionRegistry>()
}

// ── Sync deno_core ops ──────────────────────────────────────────────────

/// Allocate a new streaming compressor or decompressor and return its id.
#[op2(fast)]
fn op_compression_create(
    state: &mut OpState,
    #[string] format: &str,
    decompress: bool,
) -> Result<u32, JsErrorBox> {
    let stream = if decompress {
        match format {
            "gzip" => Stream::GzipDec(GzDecoder::new(Vec::new())),
            "deflate" => Stream::ZlibDec(ZlibDecoder::new(Vec::new())),
            "deflate-raw" => Stream::DeflateRawDec(DeflateDecoder::new(Vec::new())),
            other => {
                return Err(JsErrorBox::generic(format!(
                    "DecompressionStream: unsupported format '{}'",
                    other
                )));
            }
        }
    } else {
        match format {
            "gzip" => Stream::GzipEnc(GzEncoder::new(Vec::new(), Compression::default())),
            "deflate" => Stream::ZlibEnc(ZlibEncoder::new(Vec::new(), Compression::default())),
            "deflate-raw" => {
                Stream::DeflateRawEnc(DeflateEncoder::new(Vec::new(), Compression::default()))
            }
            other => {
                return Err(JsErrorBox::generic(format!(
                    "CompressionStream: unsupported format '{}'",
                    other
                )));
            }
        }
    };

    let reg = registry(state);
    reg.next_id = reg.next_id.wrapping_add(1);
    if reg.next_id == 0 {
        reg.next_id = 1;
    }
    let id = reg.next_id;
    reg.streams.insert(id, stream);
    Ok(id)
}

/// Push a chunk into the stream and return whatever output bytes are
/// available afterwards (may be empty).
#[op2]
#[buffer]
fn op_compression_write(
    state: &mut OpState,
    id: u32,
    #[buffer(copy)] data: Vec<u8>,
) -> Result<Vec<u8>, JsErrorBox> {
    let reg = registry(state);
    let stream = reg
        .streams
        .get_mut(&id)
        .ok_or_else(|| JsErrorBox::generic(format!("compression: invalid stream id {}", id)))?;

    let map_io = |op: &str, e: std::io::Error| {
        JsErrorBox::generic(format!("compression: {}: {}", op, e))
    };

    let drained = match stream {
        Stream::GzipEnc(e) => {
            e.write_all(&data).map_err(|err| map_io("write", err))?;
            // Sync-flush so callers see output proportional to input
            // rather than waiting for the encoder's buffer to fill.
            e.flush().map_err(|err| map_io("flush", err))?;
            std::mem::take(e.get_mut())
        }
        Stream::ZlibEnc(e) => {
            e.write_all(&data).map_err(|err| map_io("write", err))?;
            e.flush().map_err(|err| map_io("flush", err))?;
            std::mem::take(e.get_mut())
        }
        Stream::DeflateRawEnc(e) => {
            e.write_all(&data).map_err(|err| map_io("write", err))?;
            e.flush().map_err(|err| map_io("flush", err))?;
            std::mem::take(e.get_mut())
        }
        Stream::GzipDec(d) => {
            d.write_all(&data).map_err(|err| map_io("write", err))?;
            std::mem::take(d.get_mut())
        }
        Stream::ZlibDec(d) => {
            d.write_all(&data).map_err(|err| map_io("write", err))?;
            std::mem::take(d.get_mut())
        }
        Stream::DeflateRawDec(d) => {
            d.write_all(&data).map_err(|err| map_io("write", err))?;
            std::mem::take(d.get_mut())
        }
    };

    Ok(drained)
}

/// Finalize the stream: write any trailing bytes, drop the encoder,
/// return the final output.
#[op2]
#[buffer]
fn op_compression_finish(state: &mut OpState, id: u32) -> Result<Vec<u8>, JsErrorBox> {
    let reg = registry(state);
    let stream = reg
        .streams
        .remove(&id)
        .ok_or_else(|| JsErrorBox::generic(format!("compression: invalid stream id {}", id)))?;

    let map_io = |e: std::io::Error| JsErrorBox::generic(format!("compression: finish: {}", e));

    let bytes = match stream {
        Stream::GzipEnc(e) => e.finish().map_err(map_io)?,
        Stream::ZlibEnc(e) => e.finish().map_err(map_io)?,
        Stream::DeflateRawEnc(e) => e.finish().map_err(map_io)?,
        Stream::GzipDec(d) => d.finish().map_err(map_io)?,
        Stream::ZlibDec(d) => d.finish().map_err(map_io)?,
        Stream::DeflateRawDec(d) => d.finish().map_err(map_io)?,
    };
    Ok(bytes)
}

/// Cancel a stream without flushing — used when a writable is aborted.
#[op2(fast)]
fn op_compression_cancel(state: &mut OpState, id: u32) {
    if let Some(reg) = state.try_borrow_mut::<CompressionRegistry>() {
        reg.streams.remove(&id);
    }
}

// ── Extension registration ──────────────────────────────────────────────

deno_core::extension!(
    compression_ext,
    ops = [
        op_compression_create,
        op_compression_write,
        op_compression_finish,
        op_compression_cancel,
    ],
);

pub fn create_extension() -> deno_core::Extension {
    compression_ext::init()
}

// ── Inject JS wrapper into the global scope ─────────────────────────────

pub fn inject_compression(runtime: &mut JsRuntime) -> Result<(), String> {
    runtime
        .execute_script("<compression-setup>", COMPRESSION_JS_WRAPPER.to_string())
        .map_err(|e| format!("Failed to install compression wrapper: {}", e))?;
    Ok(())
}

/// Minimal `ReadableStream` / `WritableStream` plus `CompressionStream` /
/// `DecompressionStream`, sufficient for the common patterns:
///
/// - `for await (const chunk of stream) { ... }`
/// - `await reader.read()` loops
/// - `source.pipeThrough(new CompressionStream('gzip')).pipeTo(sink)`
/// - `await writer.write(chunk); await writer.close();`
///
/// The streams are NOT a complete WHATWG Streams polyfill — they ignore
/// queuing strategies / highWaterMark, don't implement BYOB readers, and
/// `tee()` is not provided. They do honor the start/pull/cancel and
/// start/write/close/abort underlying-source/sink contracts that
/// `CompressionStream` itself relies on.
const COMPRESSION_JS_WRAPPER: &str = r#"
(function() {
    var ops = Deno.core.ops;
    var op_create  = ops.op_compression_create;
    var op_write   = ops.op_compression_write;
    var op_finish  = ops.op_compression_finish;
    var op_cancel  = ops.op_compression_cancel;

    function toBytes(chunk) {
        if (chunk instanceof Uint8Array) return chunk;
        if (chunk instanceof ArrayBuffer) return new Uint8Array(chunk);
        if (ArrayBuffer.isView(chunk)) {
            return new Uint8Array(chunk.buffer, chunk.byteOffset, chunk.byteLength);
        }
        throw new TypeError('CompressionStream: chunk must be a BufferSource');
    }

    // ── ReadableStream ───────────────────────────────────────────────
    function ReadableStream(source) {
        source = source || {};
        var self = this;
        var queue = [];
        var pendingReaders = [];     // [{resolve, reject}]
        var closed = false;
        var errored = null;
        var locked = false;
        var pulling = false;

        function settle() {
            while (pendingReaders.length > 0) {
                if (errored) {
                    var r = pendingReaders.shift();
                    r.reject(errored);
                    continue;
                }
                if (queue.length > 0) {
                    var r2 = pendingReaders.shift();
                    r2.resolve({ value: queue.shift(), done: false });
                    continue;
                }
                if (closed) {
                    var r3 = pendingReaders.shift();
                    r3.resolve({ value: undefined, done: true });
                    continue;
                }
                break;
            }
        }

        function maybePull() {
            if (pulling || closed || errored) return;
            if (typeof source.pull !== 'function') return;
            if (queue.length > 0 && pendingReaders.length === 0) return;
            pulling = true;
            try {
                var p = source.pull(controller);
                Promise.resolve(p).then(function() { pulling = false; },
                                        function(e) { pulling = false; controller.error(e); });
            } catch (e) {
                pulling = false;
                controller.error(e);
            }
        }

        var controller = {
            enqueue: function(chunk) {
                if (closed || errored) throw new TypeError('ReadableStream: cannot enqueue on a closed/errored stream');
                queue.push(chunk);
                settle();
            },
            close: function() {
                if (closed || errored) return;
                closed = true;
                settle();
            },
            error: function(err) {
                if (closed || errored) return;
                errored = err;
                queue.length = 0;
                settle();
            },
            get desiredSize() {
                if (errored) return null;
                if (closed) return 0;
                return 1;
            },
        };

        Object.defineProperty(this, 'locked', { get: function() { return locked; } });

        this.cancel = function(reason) {
            if (errored) return Promise.reject(errored);
            if (closed) return Promise.resolve();
            closed = true;
            queue.length = 0;
            settle();
            try {
                var p = (typeof source.cancel === 'function') ? source.cancel(reason) : undefined;
                return Promise.resolve(p).then(function() { return undefined; });
            } catch (e) {
                return Promise.reject(e);
            }
        };

        this.getReader = function() {
            if (locked) throw new TypeError('ReadableStream: already locked');
            locked = true;
            var self2 = self;
            var released = false;
            return {
                read: function() {
                    if (released) return Promise.reject(new TypeError('ReadableStream reader released'));
                    return new Promise(function(resolve, reject) {
                        pendingReaders.push({ resolve: resolve, reject: reject });
                        settle();
                        maybePull();
                    });
                },
                cancel: function(reason) {
                    if (released) return Promise.reject(new TypeError('ReadableStream reader released'));
                    return self2.cancel(reason);
                },
                releaseLock: function() {
                    if (released) return;
                    released = true;
                    locked = false;
                },
                get closed() {
                    return new Promise(function(resolve, reject) {
                        function check() {
                            if (errored) { reject(errored); return true; }
                            if (closed) { resolve(undefined); return true; }
                            return false;
                        }
                        if (check()) return;
                        pendingReaders.push({
                            resolve: function() { if (!check()) resolve(undefined); },
                            reject: function(e) { reject(e); },
                        });
                    });
                },
            };
        };

        this[Symbol.asyncIterator] = function() {
            var reader = self.getReader();
            return {
                next: function() {
                    return reader.read().then(function(r) {
                        if (r.done) reader.releaseLock();
                        return r;
                    }, function(e) {
                        reader.releaseLock();
                        throw e;
                    });
                },
                return: function(value) {
                    reader.releaseLock();
                    return Promise.resolve({ value: value, done: true });
                },
                throw: function(e) {
                    reader.releaseLock();
                    return Promise.reject(e);
                },
                [Symbol.asyncIterator]: function() { return this; },
            };
        };

        this.pipeTo = function(dest) {
            var reader = self.getReader();
            var writer = dest.getWriter();
            return new Promise(function(resolve, reject) {
                function pump() {
                    reader.read().then(function(r) {
                        if (r.done) {
                            writer.close().then(function() {
                                reader.releaseLock();
                                writer.releaseLock();
                                resolve(undefined);
                            }, function(e) {
                                reader.releaseLock();
                                writer.releaseLock();
                                reject(e);
                            });
                            return;
                        }
                        writer.write(r.value).then(pump, function(e) {
                            reader.releaseLock();
                            writer.releaseLock();
                            reject(e);
                        });
                    }, function(e) {
                        writer.abort(e).then(function() {
                            reader.releaseLock();
                            writer.releaseLock();
                            reject(e);
                        }, function() {
                            reader.releaseLock();
                            writer.releaseLock();
                            reject(e);
                        });
                    });
                }
                pump();
            });
        };

        this.pipeThrough = function(transform) {
            self.pipeTo(transform.writable);
            return transform.readable;
        };

        // start
        if (typeof source.start === 'function') {
            try {
                var sp = source.start(controller);
                if (sp && typeof sp.then === 'function') {
                    sp.then(maybePull, function(e) { controller.error(e); });
                } else {
                    maybePull();
                }
            } catch (e) {
                controller.error(e);
            }
        } else {
            maybePull();
        }
    }

    // ── WritableStream ───────────────────────────────────────────────
    function WritableStream(sink) {
        sink = sink || {};
        var self = this;
        var queue = [];               // pending write tasks: {chunk, resolve, reject, isClose}
        var inflight = false;
        var closed = false;
        var errored = null;
        var locked = false;
        var startDone = false;
        var startPromise;
        var closedWaiters = [];       // [{resolve, reject}]

        var controller = {
            error: function(err) {
                if (errored || closed) return;
                errored = err;
                rejectAll(err);
                notifyClosed();
            },
        };

        function rejectAll(err) {
            while (queue.length > 0) {
                var task = queue.shift();
                task.reject(err);
            }
        }

        function notifyClosed() {
            while (closedWaiters.length > 0) {
                var w = closedWaiters.shift();
                if (errored) w.reject(errored); else if (closed) w.resolve(undefined);
            }
        }

        function processQueue() {
            if (inflight) return;
            if (!startDone) return;
            if (queue.length === 0) return;
            if (errored) { rejectAll(errored); return; }
            var task = queue.shift();
            inflight = true;
            try {
                var p;
                if (task.isClose) {
                    p = (typeof sink.close === 'function') ? sink.close() : undefined;
                } else {
                    p = (typeof sink.write === 'function') ? sink.write(task.chunk, controller) : undefined;
                }
                Promise.resolve(p).then(function(v) {
                    inflight = false;
                    if (task.isClose) { closed = true; notifyClosed(); }
                    task.resolve(v);
                    processQueue();
                }, function(e) {
                    inflight = false;
                    errored = errored || e;
                    task.reject(e);
                    rejectAll(errored);
                    notifyClosed();
                });
            } catch (e) {
                inflight = false;
                errored = errored || e;
                task.reject(e);
                rejectAll(errored);
                notifyClosed();
            }
        }

        try {
            var sp = (typeof sink.start === 'function') ? sink.start(controller) : undefined;
            startPromise = Promise.resolve(sp).then(function() {
                startDone = true;
                processQueue();
            }, function(e) {
                errored = e;
                startDone = true;
                rejectAll(e);
            });
        } catch (e) {
            errored = e;
            startDone = true;
            startPromise = Promise.reject(e);
        }

        Object.defineProperty(this, 'locked', { get: function() { return locked; } });

        this.abort = function(reason) {
            if (errored) return Promise.resolve();
            if (closed) return Promise.resolve();
            errored = reason !== undefined ? reason : new Error('aborted');
            rejectAll(errored);
            notifyClosed();
            try {
                var p = (typeof sink.abort === 'function') ? sink.abort(reason) : undefined;
                return Promise.resolve(p).then(function() { return undefined; });
            } catch (e) {
                return Promise.reject(e);
            }
        };

        this.close = function() {
            return getWriterInternal().close();
        };

        function getWriterInternal() {
            if (locked) throw new TypeError('WritableStream: already locked');
            locked = true;
            var released = false;
            var writer = {
                write: function(chunk) {
                    if (released) return Promise.reject(new TypeError('Writer released'));
                    if (errored) return Promise.reject(errored);
                    if (closed) return Promise.reject(new TypeError('WritableStream: already closed'));
                    return new Promise(function(resolve, reject) {
                        queue.push({ chunk: chunk, resolve: resolve, reject: reject, isClose: false });
                        processQueue();
                    });
                },
                close: function() {
                    if (released) return Promise.reject(new TypeError('Writer released'));
                    if (errored) return Promise.reject(errored);
                    if (closed) return Promise.resolve();
                    return new Promise(function(resolve, reject) {
                        queue.push({ chunk: undefined, resolve: resolve, reject: reject, isClose: true });
                        processQueue();
                    });
                },
                abort: function(reason) {
                    if (released) return Promise.reject(new TypeError('Writer released'));
                    return self.abort(reason);
                },
                releaseLock: function() {
                    if (released) return;
                    released = true;
                    locked = false;
                },
                get closed() {
                    return new Promise(function(resolve, reject) {
                        if (errored) { reject(errored); return; }
                        if (closed) { resolve(undefined); return; }
                        closedWaiters.push({ resolve: resolve, reject: reject });
                    });
                },
                get ready() {
                    return Promise.resolve(undefined);
                },
                get desiredSize() { return 1; },
            };
            return writer;
        }

        this.getWriter = getWriterInternal;
    }

    // ── CompressionStream / DecompressionStream ──────────────────────
    function makeTransform(format, decompress) {
        var id = op_create(format, decompress);
        var readableController;
        var writableClosed = false;

        var readable = new ReadableStream({
            start: function(c) { readableController = c; },
            cancel: function() {
                op_cancel(id);
            },
        });

        var writable = new WritableStream({
            write: function(chunk) {
                var bytes;
                try {
                    bytes = toBytes(chunk);
                } catch (e) {
                    op_cancel(id);
                    if (readableController) readableController.error(e);
                    throw e;
                }
                var out;
                try {
                    out = op_write(id, bytes);
                } catch (e) {
                    op_cancel(id);
                    if (readableController) readableController.error(e);
                    throw e;
                }
                if (out && out.byteLength > 0) {
                    readableController.enqueue(out);
                }
            },
            close: function() {
                if (writableClosed) return;
                writableClosed = true;
                var tail;
                try {
                    tail = op_finish(id);
                } catch (e) {
                    if (readableController) readableController.error(e);
                    throw e;
                }
                if (tail && tail.byteLength > 0) {
                    readableController.enqueue(tail);
                }
                readableController.close();
            },
            abort: function(reason) {
                if (writableClosed) return;
                writableClosed = true;
                op_cancel(id);
                if (readableController) {
                    readableController.error(reason !== undefined ? reason : new Error('aborted'));
                }
            },
        });

        return { readable: readable, writable: writable };
    }

    function CompressionStream(format) {
        if (!(this instanceof CompressionStream)) {
            throw new TypeError("CompressionStream constructor: 'new' is required");
        }
        if (format !== 'gzip' && format !== 'deflate' && format !== 'deflate-raw') {
            throw new TypeError("CompressionStream: unsupported format '" + format + "'");
        }
        var t = makeTransform(format, false);
        Object.defineProperty(this, 'readable', { value: t.readable, enumerable: true });
        Object.defineProperty(this, 'writable', { value: t.writable, enumerable: true });
    }

    function DecompressionStream(format) {
        if (!(this instanceof DecompressionStream)) {
            throw new TypeError("DecompressionStream constructor: 'new' is required");
        }
        if (format !== 'gzip' && format !== 'deflate' && format !== 'deflate-raw') {
            throw new TypeError("DecompressionStream: unsupported format '" + format + "'");
        }
        var t = makeTransform(format, true);
        Object.defineProperty(this, 'readable', { value: t.readable, enumerable: true });
        Object.defineProperty(this, 'writable', { value: t.writable, enumerable: true });
    }

    globalThis.ReadableStream = ReadableStream;
    globalThis.WritableStream = WritableStream;
    globalThis.CompressionStream = CompressionStream;
    globalThis.DecompressionStream = DecompressionStream;
})();
"#;
