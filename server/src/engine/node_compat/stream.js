// node:stream — a purpose-written subset covering the patterns gRPC-style
// libraries use for call streams: subclasses implementing _read / _write /
// _final, object mode, push(), flowing 'data' delivery, pause/resume,
// end/finish/close lifecycle, and destroy(err). Not a full Node streams
// port: no highWaterMark backpressure accounting ('drain' fires on the next
// tick after every false-returning write), no 'readable' pull mode beyond
// read(), and pipe() is a minimal data/end/error bridge.

import { EventEmitter } from 'node:events';
import { Buffer } from 'node:buffer';

function later(fn) {
    Promise.resolve().then(fn);
}

// ── Readable mixin ──────────────────────────────────────────────────────

function initReadable(self, options) {
    self._readableState = {
        objectMode: !!(options && options.objectMode),
        queue: [],
        flowing: false,
        ended: false,        // push(null) seen
        endEmitted: false,
        reading: false,
        destroyed: false,
    };
    // Attaching a 'data' listener switches to flowing mode, like Node —
    // unless the stream was explicitly paused (Node's flowing === false vs
    // null distinction).
    self.on('newListener', (event) => {
        if (event === 'data') {
            later(() => {
                if (!self._readableState.paused) self.resume();
            });
        }
    });
}

function readableMethods(proto) {
    proto.push = function push(chunk) {
        const state = this._readableState;
        if (state.destroyed) return false;
        if (chunk === null) {
            // Flush any bytes held back by the incremental decoder.
            if (state.decodePending && state.decodePending.length > 0) {
                state.queue.push(state.decodePending.toString(state.encoding));
                state.decodePending = null;
                if (state.flowing) flushReadQueue(this);
                else this.emit('readable');
            }
            state.ended = true;
            maybeEmitEnd(this);
            return false;
        }
        if (state.encoding && !state.objectMode) {
            chunk = decodeChunk(state, chunk);
            // Every byte held back for a later chunk: nothing to deliver.
            if (chunk === '') return !state.ended;
        }
        state.queue.push(chunk);
        if (state.flowing) flushReadQueue(this);
        else this.emit('readable');
        return state.queue.length === 0;
    };

    proto.read = function read() {
        const state = this._readableState;
        if (state.queue.length > 0) {
            const chunk = state.queue.shift();
            // Draining the buffer may unblock 'end' (EOF already pushed).
            if (state.queue.length === 0) maybeEmitEnd(this);
            return chunk;
        }
        callRead(this);
        maybeEmitEnd(this);
        return null;
    };

    proto.pause = function pause() {
        this._readableState.flowing = false;
        this._readableState.paused = true;
        return this;
    };

    proto.isPaused = function isPaused() {
        // Node reports paused only after an explicit pause() (flowing ===
        // false); a fresh, never-flowed stream is not paused.
        return this._readableState.paused === true;
    };

    proto.resume = function resume() {
        const state = this._readableState;
        if (state.destroyed) return this;
        state.paused = false;
        if (!state.flowing) {
            state.flowing = true;
            flushReadQueue(this);
        }
        return this;
    };

    proto.pipe = function pipe(dest) {
        this.on('data', (chunk) => dest.write(chunk));
        this.on('end', () => dest.end());
        this.on('error', (err) => dest.destroy && dest.destroy(err));
        this.resume();
        return dest;
    };

    proto[Symbol.asyncIterator] = async function* asyncIterator() {
        const state = this._readableState;
        try {
            for (;;) {
                if (state.queue.length > 0) {
                    const chunk = state.queue.shift();
                    if (state.queue.length === 0) maybeEmitEnd(this);
                    yield chunk;
                    continue;
                }
                if (state.ended || state.destroyed) {
                    if (this.errored) throw this.errored;
                    return;
                }
                // Pump a pull-based stream: without this, a stream that only
                // produces data when _read() is called would wait forever for
                // a 'readable' that never fires.
                callRead(this);
                if (state.queue.length > 0) continue;
                await new Promise((resolve, reject) => {
                    const cleanup = () => {
                        this.removeListener('readable', onWake);
                        this.removeListener('end', onWake);
                        this.removeListener('close', onWake);
                        this.removeListener('error', onError);
                    };
                    const onWake = () => { cleanup(); resolve(); };
                    const onError = (err) => { cleanup(); reject(err); };
                    this.once('readable', onWake);
                    this.once('end', onWake);
                    this.once('close', onWake);
                    this.once('error', onError);
                });
            }
        } finally {
            // Ending iteration early destroys the stream, like Node.
            if (!state.endEmitted && !state.destroyed) this.destroy();
        }
    };

    proto.setEncoding = function setEncoding(enc) {
        const state = this._readableState;
        state.encoding = normalizeEncoding(enc);
        state.decodePending = null;
        // Chunks already buffered are converted in place (no cross-chunk
        // boundary handling for data that predates the setEncoding call).
        state.queue = state.queue.map((c) =>
            typeof c === 'string' ? c : c.toString(state.encoding));
        return this;
    };
    proto.unshift = function unshift(chunk) {
        this._readableState.queue.unshift(chunk);
        return true;
    };
}

function flushReadQueue(stream) {
    const state = stream._readableState;
    later(() => {
        while (state.flowing && !state.destroyed && state.queue.length > 0) {
            stream.emit('data', state.queue.shift());
        }
        if (!state.ended && state.flowing && !state.destroyed) callRead(stream);
        maybeEmitEnd(stream);
    });
}

function callRead(stream) {
    const state = stream._readableState;
    if (state.reading || state.ended || state.destroyed) return;
    if (typeof stream._read === 'function') {
        state.reading = true;
        try {
            stream._read();
        } finally {
            state.reading = false;
        }
    }
}

function normalizeEncoding(enc) {
    const lowered = String(enc || 'utf8').toLowerCase();
    if (lowered === 'utf-8') return 'utf8';
    if (lowered === 'ucs-2' || lowered === 'ucs2') return 'ucs2';
    if (lowered === 'utf-16le') return 'utf16le';
    if (lowered === 'binary') return 'latin1';
    return lowered;
}

// How many trailing bytes of `buf` are an incomplete UTF-8 sequence.
function utf8Holdback(buf) {
    let i = buf.length - 1;
    let trailing = 0;
    while (i >= 0 && trailing < 4 && (buf[i] & 0xC0) === 0x80) {
        i--;
        trailing++;
    }
    if (i < 0) return 0;
    const lead = buf[i];
    let seqLen = 0;
    if ((lead & 0x80) === 0) seqLen = 1;
    else if ((lead & 0xE0) === 0xC0) seqLen = 2;
    else if ((lead & 0xF0) === 0xE0) seqLen = 3;
    else if ((lead & 0xF8) === 0xF0) seqLen = 4;
    else return 0; // invalid lead byte: let toString substitute
    const have = trailing + 1;
    return have < seqLen ? have : 0;
}

// StringDecoder-style incremental conversion: bytes that end mid-character
// are held back and prepended to the next chunk.
function decodeChunk(state, chunk) {
    if (typeof chunk === 'string') return chunk;
    let buf = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk);
    if (state.decodePending) {
        buf = Buffer.concat([state.decodePending, buf]);
        state.decodePending = null;
    }
    const enc = state.encoding;
    let holdback = 0;
    if (enc === 'utf8') holdback = utf8Holdback(buf);
    else if (enc === 'ucs2' || enc === 'utf16le') holdback = buf.length % 2;
    else if (enc === 'base64') holdback = buf.length % 3;
    if (holdback > 0) {
        state.decodePending = buf.slice(buf.length - holdback);
        buf = buf.slice(0, buf.length - holdback);
    }
    return buf.toString(enc);
}

function maybeEmitEnd(stream) {
    const state = stream._readableState;
    if (state.ended && !state.endEmitted && state.queue.length === 0 && !state.destroyed) {
        state.endEmitted = true;
        later(() => {
            stream.emit('end');
            maybeEmitClose(stream);
        });
    }
}

// ── Writable mixin ──────────────────────────────────────────────────────

function initWritable(self, options) {
    self._writableState = {
        objectMode: !!(options && options.objectMode),
        highWaterMark: options && options.highWaterMark !== undefined
            ? options.highWaterMark : 16384,
        pendingBytes: 0,
        queue: [],       // {chunk, encoding, callback}
        writing: false,
        ended: false,    // end() called
        finished: false,
        destroyed: false,
    };
}

function chunkSize(state, chunk) {
    if (state.objectMode) return 1;
    if (typeof chunk === 'string') return chunk.length;
    return chunk && chunk.byteLength !== undefined ? chunk.byteLength : 1;
}

function writableMethods(proto) {
    proto.write = function write(chunk, encoding, callback) {
        if (typeof encoding === 'function') {
            callback = encoding;
            encoding = undefined;
        }
        const state = this._writableState;
        if (!state.objectMode) {
            if (chunk === null) {
                const err = new TypeError('May not write null values to stream');
                err.code = 'ERR_STREAM_NULL_VALUES';
                throw err;
            }
            if (typeof chunk !== 'string' && !ArrayBuffer.isView(chunk)) {
                const err = new TypeError(
                    'The "chunk" argument must be of type string or an instance of ' +
                    `Buffer, TypedArray, or DataView. Received ${
                        chunk === undefined ? 'undefined'
                        : typeof chunk === 'object'
                            ? `an instance of ${(chunk.constructor && chunk.constructor.name) || 'Object'}`
                            : typeof chunk === 'function' ? `function ${chunk.name}`
                            : `type ${typeof chunk} (${String(chunk)})`}`);
                err.code = 'ERR_INVALID_ARG_TYPE';
                throw err;
            }
        }
        if (state.ended || state.destroyed) {
            const err = state.destroyed
                ? Object.assign(
                    new Error('Cannot call write after a stream was destroyed'),
                    { code: 'ERR_STREAM_DESTROYED' })
                : Object.assign(
                    new Error('write after end'),
                    { code: 'ERR_STREAM_WRITE_AFTER_END' });
            if (callback) later(() => callback(err));
            else this.emit('error', err);
            return false;
        }
        state.queue.push({ chunk, encoding, callback });
        state.pendingBytes += chunkSize(state, chunk);
        processWriteQueue(this);
        // Backpressure signal: false once buffered bytes reach the mark.
        return state.pendingBytes < state.highWaterMark;
    };

    proto.end = function end(chunk, encoding, callback) {
        if (typeof chunk === 'function') { callback = chunk; chunk = undefined; }
        if (typeof encoding === 'function') { callback = encoding; encoding = undefined; }
        const state = this._writableState;
        if (chunk !== undefined && chunk !== null && !state.ended && !state.destroyed) {
            state.queue.push({ chunk, encoding, callback: undefined });
        }
        state.ended = true;
        if (callback) this.once('finish', callback);
        processWriteQueue(this);
        return this;
    };

    proto.cork = function cork() {};
    proto.uncork = function uncork() {};
    proto.setDefaultEncoding = function setDefaultEncoding() { return this; };

    Object.defineProperty(proto, 'writableLength', {
        get() { return this._writableState.pendingBytes; },
        configurable: true,
    });
    Object.defineProperty(proto, 'writableEnded', {
        get() { return this._writableState.ended; },
        configurable: true,
    });
    Object.defineProperty(proto, 'writableFinished', {
        get() { return this._writableState.finished; },
        configurable: true,
    });
}

function processWriteQueue(stream) {
    const state = stream._writableState;
    if (state.writing || state.destroyed) return;
    if (state.queue.length === 0) {
        maybeFinish(stream);
        return;
    }
    const { chunk, encoding, callback } = state.queue.shift();
    state.writing = true;
    const onDone = (err) => {
        state.writing = false;
        state.pendingBytes -= chunkSize(state, chunk);
        if (state.pendingBytes < 0) state.pendingBytes = 0;
        if (callback) later(() => callback(err || null));
        if (err) {
            // A destroyed stream already routed the error through destroy();
            // emitting here again would double-report it.
            if (!callback && !stream.destroyed) stream.emit('error', err);
            return;
        }
        later(() => {
            stream.emit('drain');
            processWriteQueue(stream);
        });
    };
    try {
        if (typeof stream._write === 'function') {
            stream._write(chunk, encoding || 'utf8', onDone);
        } else {
            onDone(new Error('_write is not implemented'));
        }
    } catch (err) {
        onDone(err);
    }
}

function maybeFinish(stream) {
    const state = stream._writableState;
    if (!state.ended || state.finished || state.writing || state.destroyed) return;
    if (state.queue.length > 0) return;
    const finish = () => {
        if (state.finished || state.destroyed) return;
        state.finished = true;
        later(() => {
            stream.emit('finish');
            maybeEmitClose(stream);
        });
    };
    if (typeof stream._final === 'function') {
        try {
            stream._final((err) => {
                if (err) stream.emit('error', err);
                else finish();
            });
        } catch (err) {
            stream.emit('error', err);
        }
    } else {
        finish();
    }
}

// ── shared lifecycle ────────────────────────────────────────────────────

function maybeEmitClose(stream) {
    const readable = stream._readableState;
    const writable = stream._writableState;
    const readDone = !readable || readable.endEmitted || readable.destroyed;
    const writeDone = !writable || writable.finished || writable.destroyed;
    if (readDone && writeDone && !stream._closeEmitted) {
        stream._closeEmitted = true;
        // net.Socket 'close' carries hadError; harmless extra for others.
        later(() => stream.emit('close', false));
    }
}

function destroyImpl(stream, err) {
    if (stream.destroyed) return stream;
    stream.destroyed = true;
    if (err) stream.errored = err;
    if (stream._readableState) stream._readableState.destroyed = true;
    if (stream._writableState) stream._writableState.destroyed = true;
    const emitClose = (hadError) => {
        if (!stream._closeEmitted) {
            stream._closeEmitted = true;
            stream.emit('close', hadError);
        }
    };
    const done = (destroyErr) => {
        const finalErr = destroyErr || err;
        if (finalErr) stream.errored = finalErr;
        later(() => {
            if (finalErr) stream.emit('error', finalErr);
            emitClose(!!finalErr);
        });
    };
    if (typeof stream._destroy === 'function') {
        try {
            stream._destroy(err || null, done);
        } catch (destroyErr) {
            done(destroyErr);
        }
    } else {
        done(null);
    }
    return stream;
}

// ── public classes ──────────────────────────────────────────────────────

// Legacy base class, function-style on purpose: Node core code (and its
// vendored tests) still does `Stream.call(this)` plus prototype surgery,
// which a `class` constructor would reject. The stream module's default
// export is this constructor with the class table attached, matching
// Node's `module.exports = Stream` shape (`new (require('stream'))()`).
export function Stream(_options) {
    EventEmitter.call(this);
    this.errored = null;
}
Object.defineProperty(Stream.prototype, 'closed', {
    get() { return Boolean(this._closeEmitted); },
    configurable: true,
});
Object.setPrototypeOf(Stream.prototype, EventEmitter.prototype);
Object.setPrototypeOf(Stream, EventEmitter);

// Node's legacy-pipe prepend helper: old emitters may have prependListener
// deleted (nodejs/node locks the fallback in via test-event-emitter-prepend),
// so error handlers fall back to _events surgery.
function legacyPrepend(emitter, event, fn) {
    if (typeof emitter.prependListener === 'function') {
        return emitter.prependListener(event, fn);
    }
    if (!emitter._events || !emitter._events[event]) emitter.on(event, fn);
    else if (Array.isArray(emitter._events[event])) emitter._events[event].unshift(fn);
    else emitter._events[event] = [fn, emitter._events[event]];
}

// The legacy base's one behavior: an events-only pipe (the subclasses below
// shadow it with the subset pipe from readableMethods).
Stream.prototype.pipe = function pipe(dest, options) {
    const source = this;

    function ondata(chunk) {
        if (dest.writable && dest.write(chunk) === false && source.pause) {
            source.pause();
        }
    }
    source.on('data', ondata);

    function ondrain() {
        if (source.readable && source.resume) source.resume();
    }
    dest.on('drain', ondrain);

    let ended = false;
    function onend() {
        if (ended) return;
        ended = true;
        if (typeof dest.end === 'function') dest.end();
    }
    function onclose() {
        if (ended) return;
        ended = true;
        if (typeof dest.destroy === 'function') dest.destroy();
    }
    if (!dest._isStdio && (!options || options.end !== false)) {
        source.on('end', onend);
        source.on('close', onclose);
    }

    function onerror(err) {
        cleanup();
        if (this.listenerCount('error') === 0) this.emit('error', err);
    }
    legacyPrepend(source, 'error', onerror);
    legacyPrepend(dest, 'error', onerror);

    function cleanup() {
        source.removeListener('data', ondata);
        dest.removeListener('drain', ondrain);
        source.removeListener('end', onend);
        source.removeListener('close', onclose);
        source.removeListener('error', onerror);
        dest.removeListener('error', onerror);
        source.removeListener('end', cleanup);
        source.removeListener('close', cleanup);
        dest.removeListener('close', cleanup);
    }
    source.on('end', cleanup);
    source.on('close', cleanup);
    dest.on('close', cleanup);

    dest.emit('pipe', source);
    return dest;
};

export class Readable extends Stream {
    constructor(options) {
        super();
        this.destroyed = false;
        this._closeEmitted = false;
        initReadable(this, options);
        if (options && typeof options.read === 'function') this._read = options.read;
        if (options && typeof options.destroy === 'function') this._destroy = options.destroy;
    }
    destroy(err) { return destroyImpl(this, err); }
}
readableMethods(Readable.prototype);

export class Writable extends Stream {
    constructor(options) {
        super();
        this.destroyed = false;
        this._closeEmitted = false;
        initWritable(this, options);
        if (options && typeof options.write === 'function') this._write = options.write;
        if (options && typeof options.final === 'function') this._final = options.final;
        if (options && typeof options.destroy === 'function') this._destroy = options.destroy;
    }
    destroy(err) { return destroyImpl(this, err); }
}
writableMethods(Writable.prototype);

export class Duplex extends Stream {
    constructor(options) {
        super();
        this.destroyed = false;
        this._closeEmitted = false;
        initReadable(this, options);
        initWritable(this, options);
        if (options && typeof options.read === 'function') this._read = options.read;
        if (options && typeof options.write === 'function') this._write = options.write;
        if (options && typeof options.final === 'function') this._final = options.final;
        if (options && typeof options.destroy === 'function') this._destroy = options.destroy;
    }
    destroy(err) { return destroyImpl(this, err); }
}
readableMethods(Duplex.prototype);
writableMethods(Duplex.prototype);

export class Transform extends Duplex {
    constructor(options) {
        super(options);
        if (options && typeof options.transform === 'function') {
            this._transform = options.transform;
        }
    }
    _write(chunk, encoding, callback) {
        if (typeof this._transform !== 'function') {
            callback(new Error('_transform is not implemented'));
            return;
        }
        this._transform(chunk, encoding, (err, data) => {
            if (err) { callback(err); return; }
            if (data !== undefined && data !== null) this.push(data);
            callback();
        });
    }
    _final(callback) {
        if (typeof this._flush === 'function') {
            this._flush((err, data) => {
                if (!err && data !== undefined && data !== null) this.push(data);
                this.push(null);
                callback(err || null);
            });
        } else {
            this.push(null);
            callback();
        }
    }
}

export class PassThrough extends Transform {
    _transform(chunk, _encoding, callback) {
        callback(null, chunk);
    }
}

export function duplexPair(options) {
    const opts = options || {};
    const sides = [];
    const other = (self) => sides[sides[0] === self ? 1 : 0];
    for (let i = 0; i < 2; i++) {
        sides.push(new Duplex({
            ...opts,
            write(chunk, _encoding, callback) {
                other(this).push(chunk);
                callback();
            },
            final(callback) {
                other(this).push(null);
                callback();
            },
            read() {},
        }));
    }
    return sides;
}

export function finished(stream, callback) {
    let done = false;
    const fire = (err) => {
        if (done) return;
        done = true;
        callback(err || null);
    };
    stream.on('end', () => fire(null));
    stream.on('finish', () => fire(null));
    stream.on('close', () => fire(null));
    stream.on('error', fire);
}

export function pipeline(...args) {
    const callback = typeof args[args.length - 1] === 'function' ? args.pop() : () => {};
    let current = args[0];
    for (let i = 1; i < args.length; i++) current = current.pipe(args[i]);
    finished(current, callback);
    return current;
}

Object.assign(Stream, {
    Readable,
    Writable,
    Duplex,
    Transform,
    PassThrough,
    Stream,
    duplexPair,
    finished,
    pipeline,
});

export default Stream;
