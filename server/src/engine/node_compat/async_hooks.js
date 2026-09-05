// node:async_hooks — purpose-written subset. AsyncLocalStorage context
// propagation rides V8 promise hooks (Deno.core.setPromiseHooks), which the
// engine exposes on every isolate; timer/microtask entry points are wrapped
// on first use so scheduled callbacks re-enter the scheduling context, the
// same observable behavior as Node's AsyncContextFrame propagation.
//
// Deliberate divergences from Node:
// - createHook() emits init/before/after/promiseResolve for promises and
//   for AsyncResource scopes only; host-level resources (timers, streams,
//   sockets) do not emit lifecycle events and destroy() only fires via
//   AsyncResource.emitDestroy().
// - executionAsyncResource() returns the active promise or AsyncResource,
//   with a stable placeholder object for the root context.

function nodeError(Ctor, code, message) {
    const err = new Ctor(message);
    err.code = code;
    return err;
}

let asyncIdCounter = 1;
function newAsyncId() {
    asyncIdCounter += 1;
    return asyncIdCounter;
}

const rootResource = {};
// The active logical async context. `contextMap` carries AsyncLocalStorage
// bindings; ids mirror Node's executionAsyncId/triggerAsyncId pair.
let currentContext = {
    asyncId: 1,
    triggerAsyncId: 0,
    contextMap: new Map(),
    resource: rootResource,
};

const promiseContexts = new WeakMap();
const activeHooks = [];

function emitHook(name, ...args) {
    for (const hook of activeHooks.slice()) {
        const callback = hook[name];
        if (typeof callback !== 'function') continue;
        callback(...args);
    }
}

let installed = false;
function ensureInstalled() {
    if (installed) return;
    installed = true;
    installPromiseHooks();
    wrapScheduler('setTimeout');
    wrapScheduler('setInterval');
    wrapScheduler('setImmediate');
    wrapQueueMicrotask();
    wrapNextTick();
}

function installPromiseHooks() {
    const setPromiseHooks = globalThis.Deno?.core?.setPromiseHooks;
    if (typeof setPromiseHooks !== 'function') return;
    setPromiseHooks(
        (promise, parent) => {
            const context = {
                asyncId: newAsyncId(),
                triggerAsyncId: parent !== undefined && promiseContexts.has(parent)
                    ? promiseContexts.get(parent).asyncId
                    : currentContext.asyncId,
                contextMap: currentContext.contextMap,
                resource: promise,
            };
            promiseContexts.set(promise, context);
            if (activeHooks.length > 0) {
                emitHook('init', context.asyncId, 'PROMISE',
                    context.triggerAsyncId, promise);
            }
        },
        (promise) => {
            const context = promiseContexts.get(promise);
            if (context === undefined) return;
            context.previous = currentContext;
            currentContext = context;
            if (activeHooks.length > 0) emitHook('before', context.asyncId);
        },
        (promise) => {
            const context = promiseContexts.get(promise);
            if (context === undefined) return;
            if (activeHooks.length > 0) emitHook('after', context.asyncId);
            if (context.previous !== undefined) {
                currentContext = context.previous;
                context.previous = undefined;
            }
        },
        (promise) => {
            if (activeHooks.length === 0) return;
            const context = promiseContexts.get(promise);
            if (context !== undefined) emitHook('promiseResolve', context.asyncId);
        },
    );
}

function bindToCurrentContext(callback) {
    const scheduled = currentContext;
    return function (...args) {
        const previous = currentContext;
        currentContext = scheduled;
        try {
            return callback.apply(this, args);
        } finally {
            currentContext = previous;
        }
    };
}

function wrapScheduler(name) {
    const original = globalThis[name];
    if (typeof original !== 'function') return;
    const wrapped = function (callback, ...rest) {
        if (typeof callback !== 'function') return original(callback, ...rest);
        return original(bindToCurrentContext(callback), ...rest);
    };
    Object.defineProperty(wrapped, 'name', { value: name, configurable: true });
    globalThis[name] = wrapped;
}

function wrapQueueMicrotask() {
    const original = globalThis.queueMicrotask;
    if (typeof original !== 'function') return;
    globalThis.queueMicrotask = function queueMicrotask(callback) {
        if (typeof callback !== 'function') return original(callback);
        return original(bindToCurrentContext(callback));
    };
}

function wrapNextTick() {
    const process = globalThis.process;
    if (!process || typeof process.nextTick !== 'function') return;
    const original = process.nextTick;
    process.nextTick = function nextTick(callback, ...args) {
        if (typeof callback !== 'function') return original(callback, ...args);
        return original(bindToCurrentContext(callback), ...args);
    };
}

export function executionAsyncId() {
    return currentContext.asyncId;
}

export function triggerAsyncId() {
    return currentContext.triggerAsyncId;
}

export function executionAsyncResource() {
    return currentContext.resource;
}

const hookCallbackNames = ['init', 'before', 'after', 'destroy', 'promiseResolve'];

class AsyncHook {
    #callbacks;
    constructor(callbacks) {
        this.#callbacks = callbacks;
    }
    enable() {
        ensureInstalled();
        if (!activeHooks.includes(this.#callbacks)) {
            activeHooks.push(this.#callbacks);
        }
        return this;
    }
    disable() {
        const index = activeHooks.indexOf(this.#callbacks);
        if (index !== -1) activeHooks.splice(index, 1);
        return this;
    }
}

export function createHook(callbacks = {}) {
    for (const name of hookCallbackNames) {
        const value = callbacks[name];
        if (value !== undefined && typeof value !== 'function') {
            throw nodeError(TypeError, 'ERR_ASYNC_CALLBACK',
                `hook.${name} must be a function`);
        }
    }
    return new AsyncHook({ ...callbacks });
}

export class AsyncResource {
    #asyncId;
    #triggerAsyncId;
    #contextMap;
    constructor(type, opts = {}) {
        if (typeof type !== 'string') {
            throw nodeError(TypeError, 'ERR_INVALID_ARG_TYPE',
                'The "type" argument must be of type string');
        }
        if (typeof opts === 'number') opts = { triggerAsyncId: opts };
        let trigger = opts.triggerAsyncId;
        if (trigger === undefined) {
            trigger = currentContext.asyncId;
        } else if (typeof trigger !== 'number' || trigger < -1 ||
                   !Number.isInteger(trigger)) {
            throw nodeError(RangeError, 'ERR_INVALID_ASYNC_ID',
                `Invalid triggerAsyncId value: ${trigger}`);
        }
        ensureInstalled();
        this.#asyncId = newAsyncId();
        this.#triggerAsyncId = trigger;
        this.#contextMap = currentContext.contextMap;
        if (activeHooks.length > 0) {
            emitHook('init', this.#asyncId, type, this.#triggerAsyncId, this);
        }
    }

    asyncId() {
        return this.#asyncId;
    }

    triggerAsyncId() {
        return this.#triggerAsyncId;
    }

    runInAsyncScope(fn, thisArg, ...args) {
        const previous = currentContext;
        currentContext = {
            asyncId: this.#asyncId,
            triggerAsyncId: this.#triggerAsyncId,
            contextMap: this.#contextMap,
            resource: this,
        };
        if (activeHooks.length > 0) emitHook('before', this.#asyncId);
        try {
            return fn.apply(thisArg, args);
        } finally {
            if (activeHooks.length > 0) emitHook('after', this.#asyncId);
            currentContext = previous;
        }
    }

    emitDestroy() {
        if (activeHooks.length > 0) emitHook('destroy', this.#asyncId);
        return this;
    }

    bind(fn, ...thisArgHolder) {
        if (typeof fn !== 'function') {
            throw nodeError(TypeError, 'ERR_INVALID_ARG_TYPE',
                'The "fn" argument must be of type function');
        }
        const resource = this;
        // An omitted thisArg forwards the call-site `this` (Node's contract:
        // an explicit undefined pins `this` to undefined instead).
        const bound = thisArgHolder.length > 0
            ? function (...args) {
                return resource.runInAsyncScope(fn, thisArgHolder[0], ...args);
            }
            : function (...args) {
                return resource.runInAsyncScope(fn, this, ...args);
            };
        Object.defineProperty(bound, 'asyncResource', {
            value: resource,
            configurable: true,
            enumerable: true,
            writable: true,
        });
        Object.defineProperty(bound, 'length', {
            value: fn.length,
            configurable: true,
        });
        return bound;
    }

    static bind(fn, type, ...thisArgHolder) {
        const resource = new AsyncResource(
            type || fn?.name || 'bound-anonymous-fn');
        return resource.bind(fn, ...thisArgHolder);
    }
}

export class AsyncLocalStorage {
    #defaultValue;
    #name;
    constructor(options = {}) {
        if (options === null || typeof options !== 'object') {
            throw nodeError(TypeError, 'ERR_INVALID_ARG_TYPE',
                'The "options" argument must be of type object');
        }
        if (options.name !== undefined && typeof options.name !== 'string') {
            throw nodeError(TypeError, 'ERR_INVALID_ARG_TYPE',
                'The "options.name" property must be of type string');
        }
        this.#defaultValue = options.defaultValue;
        this.#name = options.name === undefined ? '' : options.name;
        ensureInstalled();
    }

    get name() {
        return this.#name;
    }

    run(store, callback, ...args) {
        const previous = currentContext;
        const contextMap = new Map(previous.contextMap);
        contextMap.set(this, store);
        currentContext = { ...previous, contextMap };
        try {
            return callback(...args);
        } finally {
            currentContext = previous;
        }
    }

    exit(callback, ...args) {
        const previous = currentContext;
        const contextMap = new Map(previous.contextMap);
        contextMap.delete(this);
        currentContext = { ...previous, contextMap };
        try {
            return callback(...args);
        } finally {
            currentContext = previous;
        }
    }

    enterWith(store) {
        const contextMap = new Map(currentContext.contextMap);
        contextMap.set(this, store);
        currentContext = { ...currentContext, contextMap };
    }

    disable() {
        const contextMap = new Map(currentContext.contextMap);
        contextMap.delete(this);
        currentContext = { ...currentContext, contextMap };
    }

    getStore() {
        if (currentContext.contextMap.has(this)) {
            return currentContext.contextMap.get(this);
        }
        return this.#defaultValue;
    }

    static bind(fn) {
        return AsyncResource.bind(fn, 'bound-anonymous-fn');
    }

    static snapshot() {
        const snapshotContext = currentContext;
        return function (callback, ...args) {
            const previous = currentContext;
            currentContext = {
                ...previous,
                contextMap: snapshotContext.contextMap,
            };
            try {
                return callback(...args);
            } finally {
                currentContext = previous;
            }
        };
    }
}

export default {
    AsyncLocalStorage,
    AsyncResource,
    createHook,
    executionAsyncId,
    executionAsyncResource,
    triggerAsyncId,
};
