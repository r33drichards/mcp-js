// node:perf_hooks — user timing and function timing over the shared web
// performance timeline. Runtime lifecycle metrics and histograms require
// host support and are intentionally not exposed here.

const bridge = globalThis.__mcpV8Performance;

function invalidArgType(name, expected, value) {
    const error = new TypeError(
        `The "${name}" argument must be of type ${expected}. Received ${value === null ? 'null' : typeof value}`,
    );
    error.code = 'ERR_INVALID_ARG_TYPE';
    return error;
}

function validateOptions(options) {
    if (options === undefined) return {};
    if (options === null || typeof options !== 'object') {
        throw invalidArgType('options', 'object', options);
    }
    return options;
}

export class PerformanceObserverEntryList {
    constructor(entries) {
        this._entries = entries.slice().sort((a, b) => a.startTime - b.startTime);
    }
    getEntries() { return this._entries.slice(); }
    getEntriesByType(type) {
        type = String(type);
        return this._entries.filter((entry) => entry.entryType === type);
    }
    getEntriesByName(name, type) {
        name = String(name);
        return this._entries.filter((entry) =>
            entry.name === name && (type === undefined || entry.entryType === String(type)));
    }
}

const supportedEntryTypes = Object.freeze([
    'dns', 'function', 'gc', 'http', 'http2', 'mark', 'measure', 'net', 'resource',
]);

export class PerformanceObserver {
    constructor(callback) {
        if (typeof callback !== 'function') {
            throw invalidArgType('callback', 'function', callback);
        }
        this._callback = callback;
        this._entryTypes = new Set();
        this._records = [];
        this._scheduled = false;
        this._unsubscribe = null;
    }

    observe(options) {
        options = validateOptions(options);
        if (!Array.isArray(options.entryTypes)) {
            throw invalidArgType('options.entryTypes', 'Array', options.entryTypes);
        }
        this.disconnect();
        for (const type of options.entryTypes) this._entryTypes.add(String(type));
        this._unsubscribe = bridge.subscribe((entry) => {
            if (!this._entryTypes.has(entry.entryType)) return;
            this._records.push(entry);
            if (this._scheduled) return;
            this._scheduled = true;
            setImmediate(() => {
                this._scheduled = false;
                if (this._records.length === 0) return;
                const records = this.takeRecords();
                this._callback(new PerformanceObserverEntryList(records), this);
            });
        });
    }

    disconnect() {
        if (this._unsubscribe) this._unsubscribe();
        this._unsubscribe = null;
        this._entryTypes.clear();
        this._records.length = 0;
    }

    takeRecords() {
        const records = this._records.slice();
        this._records.length = 0;
        return records;
    }

    static get supportedEntryTypes() { return supportedEntryTypes; }
}

function emitFunctionEntry(fn, start, args) {
    const entry = bridge.createEntry(
        fn.name || '', 'function', start, performance.now() - start, args.slice(),
    );
    args.forEach((value, index) => { entry[index] = value; });
    bridge.emit(entry);
}

export function timerify(fn, options) {
    if (typeof fn !== 'function') throw invalidArgType('fn', 'function', fn);
    options = validateOptions(options);
    if (options.histogram !== undefined) {
        throw invalidArgType('options.histogram', 'RecordableHistogram', options.histogram);
    }

    function timerified(...args) {
        const start = performance.now();
        if (new.target) {
            const value = Reflect.construct(fn, args, fn);
            emitFunctionEntry(fn, start, args);
            return value;
        }
        const value = Reflect.apply(fn, this, args);
        if (value !== null && (typeof value === 'object' || typeof value === 'function') &&
            typeof value.finally === 'function') {
            return value.finally(() => emitFunctionEntry(fn, start, args));
        }
        emitFunctionEntry(fn, start, args);
        return value;
    }
    Object.defineProperty(timerified, 'name', {
        value: `timerified ${fn.name || ''}`.trimEnd(), configurable: true,
    });
    Object.defineProperty(timerified, 'length', { value: fn.length, configurable: true });
    return timerified;
}

export const performance = globalThis.performance;
export const Performance = globalThis.Performance;
export const PerformanceEntry = globalThis.PerformanceEntry;
export const PerformanceMark = globalThis.PerformanceMark;
export const PerformanceMeasure = globalThis.PerformanceMeasure;

Object.defineProperty(performance, 'timerify', {
    value: timerify, writable: true, configurable: true,
});

const perfHooks = {
    Performance,
    PerformanceEntry,
    PerformanceMark,
    PerformanceMeasure,
    PerformanceObserver,
    PerformanceObserverEntryList,
    performance,
    timerify,
};

export default perfHooks;
