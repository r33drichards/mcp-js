// node:stream — a purpose-written subset covering the patterns gRPC-style
// libraries use for call streams: subclasses implementing _read / _write /
// _final, object mode, push(), flowing 'data' delivery, pause/resume,
// end/finish/close lifecycle, and destroy(err). Not a full Node streams
// port: no highWaterMark backpressure accounting ('drain' fires on the next
// tick after every false-returning write), no 'readable' pull mode beyond
// read(), and pipe() is a minimal data/end/error bridge.

import { EventEmitter } from 'node:events';

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
    // Attaching a 'data' listener switches to flowing mode, like Node.
    self.on('newListener', (event) => {
        if (event === 'data') later(() => self.resume());
    });
}

function readableMethods(proto) {
    proto.push = function push(chunk) {
        const state = this._readableState;
        if (state.destroyed) return false;
        if (chunk === null) {
            state.ended = true;
            maybeEmitEnd(this);
            return false;
        }
        state.queue.push(chunk);
        if (state.flowing) flushReadQueue(this);
        else this.emit('readable');
        return state.queue.length === 0;
    };

    proto.read = function read() {
        const state = this._readableState;
        if (state.queue.length > 0) return state.queue.shift();
        callRead(this);
        maybeEmitEnd(this);
        return null;
    };

    proto.pause = function pause() {
        this._readableState.flowing = false;
        return this;
    };

    proto.isPaused = function isPaused() {
        return !this._readableState.flowing;
    };

    proto.resume = function resume() {
        const state = this._readableState;
        if (state.destroyed) return this;
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

    proto.setEncoding = function setEncoding() { return this; };
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
        queue: [],       // {chunk, encoding, callback}
        writing: false,
        ended: false,    // end() called
        finished: false,
        destroyed: false,
    };
}

function writableMethods(proto) {
    proto.write = function write(chunk, encoding, callback) {
        if (typeof encoding === 'function') {
            callback = encoding;
            encoding = undefined;
        }
        const state = this._writableState;
        if (state.ended || state.destroyed) {
            const err = new Error('write after end');
            if (callback) later(() => callback(err));
            else this.emit('error', err);
            return false;
        }
        state.queue.push({ chunk, encoding, callback });
        processWriteQueue(this);
        return true;
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
        if (callback) later(() => callback(err || null));
        if (err) {
            if (!callback) stream.emit('error', err);
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
        later(() => stream.emit('close'));
    }
}

function destroyImpl(stream, err) {
    if (stream.destroyed) return stream;
    stream.destroyed = true;
    if (stream._readableState) stream._readableState.destroyed = true;
    if (stream._writableState) stream._writableState.destroyed = true;
    const emitClose = () => {
        if (!stream._closeEmitted) {
            stream._closeEmitted = true;
            stream.emit('close');
        }
    };
    const done = (destroyErr) => {
        const finalErr = destroyErr || err;
        later(() => {
            if (finalErr) stream.emit('error', finalErr);
            emitClose();
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

export class Readable extends EventEmitter {
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

export class Writable extends EventEmitter {
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

export class Duplex extends EventEmitter {
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

export default {
    Readable,
    Writable,
    Duplex,
    Transform,
    PassThrough,
    Stream: Readable,
    finished,
    pipeline,
};
