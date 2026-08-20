// node:util — the commonly-used subset: format/inspect (delegating to a
// compact inspector), promisify/callbackify, types checks, inherits,
// TextEncoder/TextDecoder re-exports, deprecate, debuglog.

function inspect(value, options) {
    const opts = typeof options === 'object' && options !== null ? options : {};
    const depth = opts.depth === undefined ? 2 : opts.depth;
    // Unlike console formatting, util.inspect quotes top-level strings.
    if (typeof value === 'string') return `'${value}'`;
    return inspectValue(value, 0, depth === null ? Infinity : depth, []);
}
inspect.custom = Symbol.for('nodejs.util.inspect.custom');

function inspectValue(value, depth, maxDepth, seen) {
    switch (typeof value) {
        case 'undefined': return 'undefined';
        case 'boolean': case 'number': return Object.is(value, -0) ? '-0' : String(value);
        case 'bigint': return String(value) + 'n';
        case 'symbol': return value.toString();
        case 'function': {
            const kind = value.constructor && value.constructor.name === 'AsyncFunction'
                ? 'AsyncFunction' : 'Function';
            return `[${kind}${value.name ? ': ' + value.name : ' (anonymous)'}]`;
        }
        case 'string':
            return depth === 0 ? value : `'${value}'`;
    }
    if (value === null) return 'null';
    if (seen.includes(value)) return '[Circular *1]';
    if (depth > maxDepth) return Array.isArray(value) ? '[Array]' : '[Object]';
    seen = seen.concat([value]);
    const next = depth + 1;
    if (typeof value[inspect.custom] === 'function') {
        try {
            return String(value[inspect.custom](
                maxDepth - depth,
                {},
                (nested, nestedOptions) => inspect(nested, nestedOptions),
            ));
        } catch { /* fall through */ }
    }
    if (Array.isArray(value)) {
        const items = value.slice(0, 100).map((v) => inspectValue(v, next, maxDepth, seen));
        if (value.length > 100) items.push(`... ${value.length - 100} more items`);
        return `[ ${items.join(', ')} ]`;
    }
    if (value instanceof Date) return value.toISOString();
    if (value instanceof RegExp) return value.toString();
    if (value instanceof Error) return value.stack || `${value.name}: ${value.message}`;
    if (ArrayBuffer.isView(value) && !(value instanceof DataView)) {
        const tag = value.constructor.name;
        const shown = Array.from(value.subarray ? value.subarray(0, 100) : value).map(String);
        if (value.length > 100) shown.push(`... ${value.length - 100} more items`);
        return shown.length === 0
            ? `${tag}(${value.length}) []`
            : `${tag}(${value.length}) [ ${shown.join(', ')} ]`;
    }
    if (value instanceof Map) {
        const entries = [];
        for (const [k, v] of value) {
            if (entries.length >= 100) { entries.push('...'); break; }
            entries.push(`${inspectValue(k, next, maxDepth, seen)} => ${inspectValue(v, next, maxDepth, seen)}`);
        }
        return `Map(${value.size}) { ${entries.join(', ')} }`;
    }
    if (value instanceof Set) {
        const entries = [];
        for (const v of value) {
            if (entries.length >= 100) { entries.push('...'); break; }
            entries.push(inspectValue(v, next, maxDepth, seen));
        }
        return `Set(${value.size}) { ${entries.join(', ')} }`;
    }
    const keys = Object.keys(value);
    const props = keys.slice(0, 100).map((k) => {
        try { return `${k}: ${inspectValue(value[k], next, maxDepth, seen)}`; }
        catch { return `${k}: [Getter threw]`; }
    });
    if (keys.length > 100) props.push(`... ${keys.length - 100} more`);
    const ctor = value.constructor && value.constructor.name;
    const prefix = ctor && ctor !== 'Object' ? ctor + ' ' : '';
    return props.length === 0 ? `${prefix}{}` : `${prefix}{ ${props.join(', ')} }`;
}

function format(f, ...args) {
    if (typeof f !== 'string') {
        return [f, ...args].map((a) => inspectValue(a, 0, 2, [])).join(' ');
    }
    let i = 0;
    let out = f.replace(/%[sdifjoO%]/g, (m) => {
        if (m === '%%') return '%';
        if (i >= args.length) return m;
        const a = args[i++];
        switch (m[1]) {
            case 's': return typeof a === 'string' ? a : inspectValue(a, 0, 2, []);
            case 'd': return typeof a === 'bigint' ? String(a) + 'n' : String(Number(a));
            case 'i': return String(parseInt(a, 10));
            case 'f': return String(parseFloat(a));
            case 'j': try { return JSON.stringify(a); } catch { return '[Circular]'; }
            case 'o': case 'O': return inspectValue(a, 1, 4, []);
            default: return m;
        }
    });
    for (; i < args.length; i++) {
        out += ' ' + inspectValue(args[i], 0, 2, []);
    }
    return out;
}

function promisify(original) {
    if (typeof original !== 'function') {
        throw new TypeError('The "original" argument must be of type function');
    }
    if (original[promisify.custom]) return original[promisify.custom];
    function fn(...args) {
        return new Promise((resolve, reject) => {
            original.call(this, ...args, (err, ...values) => {
                if (err) reject(err);
                else resolve(values.length > 1 ? values : values[0]);
            });
        });
    }
    Object.setPrototypeOf(fn, Object.getPrototypeOf(original));
    Object.defineProperty(fn, 'name', { value: original.name, configurable: true });
    return fn;
}
promisify.custom = Symbol.for('nodejs.util.promisify.custom');

function callbackify(original) {
    if (typeof original !== 'function') {
        throw new TypeError('The "original" argument must be of type function');
    }
    function callbackified(...args) {
        const cb = args.pop();
        if (typeof cb !== 'function') {
            throw new TypeError('The last argument must be of type function');
        }
        Promise.resolve(original.apply(this, args)).then(
            (ret) => queueMicrotask(() => cb(null, ret)),
            (err) => queueMicrotask(() => cb(err || new Error('Promise was rejected with falsy value'))),
        );
    }
    Object.defineProperty(callbackified, 'name', { value: original.name, configurable: true });
    return callbackified;
}

function inherits(ctor, superCtor) {
    Object.defineProperty(ctor, 'super_', { value: superCtor, writable: true, configurable: true });
    Object.setPrototypeOf(ctor.prototype, superCtor.prototype);
}

function deprecate(fn, msg) {
    let warned = false;
    function deprecated(...args) {
        if (!warned) { warned = true; console.warn(msg); }
        return fn.apply(this, args);
    }
    return deprecated;
}

function debuglog() {
    return function debug() {};
}

const types = {
    isDate: (v) => v instanceof Date,
    isRegExp: (v) => v instanceof RegExp,
    isNativeError: (v) => v instanceof Error,
    isPromise: (v) => v instanceof Promise,
    isMap: (v) => v instanceof Map,
    isSet: (v) => v instanceof Set,
    isWeakMap: (v) => v instanceof WeakMap,
    isWeakSet: (v) => v instanceof WeakSet,
    isArrayBuffer: (v) => v instanceof ArrayBuffer,
    isSharedArrayBuffer: (v) => typeof SharedArrayBuffer === 'function' && v instanceof SharedArrayBuffer,
    isAnyArrayBuffer: (v) => types.isArrayBuffer(v) || types.isSharedArrayBuffer(v),
    isArrayBufferView: (v) => ArrayBuffer.isView(v),
    isTypedArray: (v) => ArrayBuffer.isView(v) && !(v instanceof DataView),
    isDataView: (v) => v instanceof DataView,
    isUint8Array: (v) => v instanceof Uint8Array,
    isAsyncFunction: (v) => typeof v === 'function' && v.constructor && v.constructor.name === 'AsyncFunction',
    isGeneratorFunction: (v) => typeof v === 'function' && v.constructor && v.constructor.name === 'GeneratorFunction',
    isProxy: () => false,
    isBoxedPrimitive: (v) => {
        const t = Object.prototype.toString.call(v);
        return typeof v === 'object' && v !== null &&
            ['[object Boolean]', '[object Number]', '[object String]', '[object BigInt]', '[object Symbol]'].includes(t);
    },
};

const util = {
    format, inspect, promisify, callbackify, inherits, deprecate, debuglog,
    types,
    TextEncoder: globalThis.TextEncoder,
    TextDecoder: globalThis.TextDecoder,
};

export { format, inspect, promisify, callbackify, inherits, deprecate, debuglog, types };
export const TextEncoder = globalThis.TextEncoder;
export const TextDecoder = globalThis.TextDecoder;
export default util;
