// node:diagnostics_channel — purpose-written subset following Node's
// lib/diagnostics_channel.js semantics.
//
// Deliberate divergences from Node:
// - Subscriber exceptions rethrow on a fresh macrotask instead of running
//   the uncaughtException machinery (the sandbox has no process-level
//   exception capture); publish() still notifies every subscriber first.
// - Channel registry entries are dropped only when a channel becomes
//   unreferenced AND garbage collection actually reclaims it, same as Node;
//   the WeakReference emulation below pins actively-used channels.

function nodeError(Ctor, code, message) {
    const err = new Ctor(message);
    err.code = code;
    return err;
}

function validateFunction(value, name) {
    if (typeof value !== 'function') {
        throw nodeError(TypeError, 'ERR_INVALID_ARG_TYPE',
            `The "${name}" argument must be of type function` +
            receivedSuffix(value));
    }
}

function receivedSuffix(value) {
    if (value === null) return '. Received null';
    if (value === undefined) return '. Received undefined';
    if (typeof value === 'object') {
        const name = value.constructor && value.constructor.name;
        return name ? `. Received an instance of ${name}` : '. Received an object';
    }
    return `. Received type ${typeof value} (${String(value)})`;
}

// Mirrors Node's internal WeakReference: weak by default, pinned strongly
// while incRef holders (subscriptions, bound stores) keep it alive.
class WeakReference {
    #weak;
    #strong = null;
    #refs = 0;
    constructor(value) {
        this.#weak = new WeakRef(value);
    }
    get() {
        return this.#weak.deref();
    }
    incRef() {
        this.#refs += 1;
        if (this.#refs === 1) this.#strong = this.get() ?? null;
        return this.#refs;
    }
    decRef() {
        this.#refs -= 1;
        if (this.#refs === 0) this.#strong = null;
        return this.#refs;
    }
}

const channels = new Map();

function rethrowUncaught(err) {
    setTimeout(() => { throw err; }, 0);
}

export class Channel {
    #subscribers = [];
    #stores = new Map();
    constructor(name) {
        this.name = name;
    }

    get hasSubscribers() {
        return this.#subscribers.length > 0 || this.#stores.size > 0;
    }

    subscribe(subscription) {
        validateFunction(subscription, 'subscription');
        this.#subscribers.push(subscription);
        channels.get(this.name)?.incRef();
    }

    unsubscribe(subscription) {
        const index = this.#subscribers.indexOf(subscription);
        if (index === -1) return false;
        this.#subscribers.splice(index, 1);
        channels.get(this.name)?.decRef();
        return true;
    }

    bindStore(store, transform) {
        if (transform !== undefined) validateFunction(transform, 'transform');
        if (!this.#stores.has(store)) channels.get(this.name)?.incRef();
        this.#stores.set(store, transform);
    }

    unbindStore(store) {
        if (!this.#stores.has(store)) return false;
        this.#stores.delete(store);
        channels.get(this.name)?.decRef();
        return true;
    }

    publish(data) {
        const subscribers = this.#subscribers.slice();
        for (const subscription of subscribers) {
            try {
                subscription(data, this.name);
            } catch (err) {
                rethrowUncaught(err);
            }
        }
    }

    runStores(data, fn, thisArg, ...args) {
        let run = () => {
            this.publish(data);
            return fn.apply(thisArg, args);
        };
        for (const [store, transform] of this.#stores.entries()) {
            const next = run;
            run = () => {
                let context = data;
                if (transform !== undefined) {
                    try {
                        context = transform(data);
                    } catch (err) {
                        rethrowUncaught(err);
                    }
                }
                return store.run(context, next);
            };
        }
        return run();
    }
}

function validateChannelName(name) {
    if (typeof name !== 'string' && typeof name !== 'symbol') {
        throw nodeError(TypeError, 'ERR_INVALID_ARG_TYPE',
            'The "channel" argument must be of type string or symbol' +
            receivedSuffix(name));
    }
}

export function channel(name) {
    const ref = channels.get(name);
    const existing = ref === undefined ? undefined : ref.get();
    if (existing !== undefined) return existing;
    validateChannelName(name);
    const created = new Channel(name);
    channels.set(name, new WeakReference(created));
    return created;
}

export function subscribe(name, subscription) {
    return channel(name).subscribe(subscription);
}

export function unsubscribe(name, subscription) {
    return channel(name).unsubscribe(subscription);
}

export function hasSubscribers(name) {
    const ref = channels.get(name);
    const existing = ref === undefined ? undefined : ref.get();
    return existing !== undefined && existing.hasSubscribers;
}

const traceEvents = ['start', 'end', 'asyncStart', 'asyncEnd', 'error'];

class TracingChannel {
    constructor(nameOrChannels) {
        if (typeof nameOrChannels === 'string') {
            for (const eventName of traceEvents) {
                this[eventName] = channel(`tracing:${nameOrChannels}:${eventName}`);
            }
            return;
        }
        if (typeof nameOrChannels === 'object' && nameOrChannels !== null) {
            for (const eventName of traceEvents) {
                const value = nameOrChannels[eventName];
                if (!(value instanceof Channel)) {
                    throw nodeError(TypeError, 'ERR_INVALID_ARG_TYPE',
                        `The "nameOrChannels.${eventName}" property must be an ` +
                        'instance of Channel' + receivedSuffix(value));
                }
                this[eventName] = value;
            }
            return;
        }
        throw nodeError(TypeError, 'ERR_INVALID_ARG_TYPE',
            'The "nameOrChannels" argument must be of type string or an ' +
            'instance of Channel or TracingChannel' + receivedSuffix(nameOrChannels));
    }

    get hasSubscribers() {
        return this.start.hasSubscribers ||
            this.end.hasSubscribers ||
            this.asyncStart.hasSubscribers ||
            this.asyncEnd.hasSubscribers ||
            this.error.hasSubscribers;
    }

    subscribe(handlers) {
        for (const eventName of traceEvents) {
            if (!handlers[eventName]) continue;
            this[eventName].subscribe(handlers[eventName]);
        }
    }

    unsubscribe(handlers) {
        let done = true;
        for (const eventName of traceEvents) {
            if (!handlers[eventName]) continue;
            if (!this[eventName].unsubscribe(handlers[eventName])) done = false;
        }
        return done;
    }

    traceSync(fn, context = {}, thisArg, ...args) {
        if (!this.hasSubscribers) return fn.apply(thisArg, args);
        const { start, end, error } = this;
        return start.runStores(context, () => {
            try {
                const result = fn.apply(thisArg, args);
                context.result = result;
                return result;
            } catch (err) {
                context.error = err;
                error.publish(context);
                throw err;
            } finally {
                end.publish(context);
            }
        });
    }

    tracePromise(fn, context = {}, thisArg, ...args) {
        if (!this.hasSubscribers) return fn.apply(thisArg, args);
        const { start, end, asyncStart, asyncEnd, error } = this;

        const trace = (promise) => promise.then(
            (result) => {
                context.result = result;
                asyncStart.publish(context);
                asyncEnd.publish(context);
                return result;
            },
            (err) => {
                context.error = err;
                error.publish(context);
                asyncStart.publish(context);
                asyncEnd.publish(context);
                throw err;
            },
        );

        return start.runStores(context, () => {
            try {
                let result = fn.apply(thisArg, args);
                if (result && typeof result.then === 'function') {
                    result = trace(result);
                }
                return result;
            } catch (err) {
                context.error = err;
                error.publish(context);
                throw err;
            } finally {
                end.publish(context);
            }
        });
    }

    traceCallback(fn, position = -1, context = {}, thisArg, ...args) {
        if (!this.hasSubscribers) return fn.apply(thisArg, args);
        const { start, end, asyncStart, asyncEnd, error } = this;

        const wrap = (callback) => {
            validateFunction(callback, 'callback');
            return (err, ...callbackArgs) => {
                if (err) {
                    context.error = err;
                    error.publish(context);
                } else {
                    context.result = callbackArgs[0];
                }
                return asyncStart.runStores(context, () => {
                    try {
                        return callback(err, ...callbackArgs);
                    } finally {
                        asyncEnd.publish(context);
                    }
                });
            };
        };

        if (position >= 0) {
            const index = position < args.length ? position : args.length;
            args[index] = wrap(args[index]);
        }

        return start.runStores(context, () => {
            try {
                return fn.apply(thisArg, args);
            } catch (err) {
                context.error = err;
                error.publish(context);
                throw err;
            } finally {
                end.publish(context);
            }
        });
    }
}

export function tracingChannel(nameOrChannels) {
    return new TracingChannel(nameOrChannels);
}

export default {
    channel,
    hasSubscribers,
    subscribe,
    tracingChannel,
    unsubscribe,
    Channel,
};
