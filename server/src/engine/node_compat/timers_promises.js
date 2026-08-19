// node:timers/promises — promisified timers over the runtime timer
// globals. Supports the options Node-targeting packages actually pass:
// `signal` (AbortSignal cancellation) everywhere; `ref` is accepted and
// ignored (the execution timeout owns the event-loop lifetime).
//
// Deliberate divergence: the setInterval() async iterator waits per
// iteration instead of buffering missed ticks, so a slow consumer sees
// interval-spaced yields rather than a burst of queued ones.

const g = globalThis;

function abortError(signal) {
    // Node rejects with the signal's reason when one was provided.
    if (signal.reason !== undefined) return signal.reason;
    const err = new Error('The operation was aborted');
    err.name = 'AbortError';
    err.code = 'ABORT_ERR';
    return err;
}

function checkSignal(signal, method) {
    if (signal === undefined || signal instanceof AbortSignal) return;
    const err = new TypeError(
        'The "options.signal" argument passed to ' + method +
        ' must be an instance of AbortSignal');
    err.code = 'ERR_INVALID_ARG_TYPE';
    throw err;
}

export function setTimeout(delay, value, options = {}) {
    const { signal } = options;
    return new Promise((resolve, reject) => {
        checkSignal(signal, 'timers/promises setTimeout');
        if (signal && signal.aborted) return reject(abortError(signal));
        const id = g.setTimeout(() => {
            if (signal) signal.removeEventListener('abort', onAbort);
            resolve(value);
        }, delay);
        function onAbort() {
            g.clearTimeout(id);
            reject(abortError(signal));
        }
        if (signal) signal.addEventListener('abort', onAbort, { once: true });
    });
}

export function setImmediate(value, options = {}) {
    const { signal } = options;
    return new Promise((resolve, reject) => {
        checkSignal(signal, 'timers/promises setImmediate');
        if (signal && signal.aborted) return reject(abortError(signal));
        const id = g.setImmediate(() => {
            if (signal) signal.removeEventListener('abort', onAbort);
            resolve(value);
        });
        function onAbort() {
            g.clearImmediate(id);
            reject(abortError(signal));
        }
        if (signal) signal.addEventListener('abort', onAbort, { once: true });
    });
}

export async function* setInterval(delay, value, options = {}) {
    const { signal } = options;
    checkSignal(signal, 'timers/promises setInterval');
    while (true) {
        await setTimeout(delay, undefined, { signal });
        yield value;
    }
}

export const scheduler = {
    wait(delay, options) {
        return setTimeout(delay, undefined, options);
    },
    yield() {
        return setImmediate();
    },
};

export default { setTimeout, setImmediate, setInterval, scheduler };
