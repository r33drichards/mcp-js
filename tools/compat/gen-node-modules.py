#!/usr/bin/env python3
"""Generate self-contained ESM builds of Node.js core modules.

Takes a directory containing Node's lib/ sources (fetched at a pinned tag)
and emits one ESM file per module into server/src/engine/node_compat/gen/.
The Node sources run VERBATIM inside a CJS-style wrapper; the `primordials`
identifiers they reference are provided by a generated runtime shim
(uncurried prototype methods etc.), and `require('internal/...')` resolves
to small stubs defined in the same file. No source transformation, so
refreshing to a newer Node tag is mechanical.

Usage: gen-node-modules.py /path/to/node-src
where node-src contains lib/path.js, lib/querystring.js, lib/events.js,
lib/internal/{constants,querystring,fixed_queue}.js and VERSION.
"""

import re
import sys
import json
import pathlib

SRC = pathlib.Path(sys.argv[1])
REPO = pathlib.Path(__file__).resolve().parents[2]
OUT = REPO / "server/src/engine/node_compat/gen"
OUT.mkdir(parents=True, exist_ok=True)
VERSION = (SRC / "VERSION").read_text().strip()

GLOBALS = (
    "String|Array|Object|Number|Math|JSON|Reflect|Symbol|Promise|Date|Error|"
    "BigInt|Boolean|Function|RegExp|Proxy|WeakMap|WeakSet|Map|Set|ArrayBuffer|"
    "DataView|TypeError|RangeError|EvalError|URIError|SyntaxError|ReferenceError|"
    "AggregateError|WeakRef|FinalizationRegistry|Uint8Array|Int8Array|Uint16Array|"
    "Int16Array|Uint32Array|Int32Array|Float32Array|Float64Array|BigInt64Array|"
    "BigUint64Array|Uint8ClampedArray|TypedArray"
)

SPECIAL = {
    "SafeMap": "Map",
    "SafeSet": "Set",
    "SafeWeakMap": "WeakMap",
    "SafeWeakSet": "WeakSet",
    "SafeFinalizationRegistry": "FinalizationRegistry",
    "SafeWeakRef": "WeakRef",
    "SafePromiseAll": "((values) => Promise.all(values))",
    "SafeArrayIterator": "(function SafeArrayIterator(a) { return a[Symbol.iterator](); })",
    "SafeStringIterator": "(function SafeStringIterator(s) { return s[Symbol.iterator](); })",
    "uncurryThis": "__uncurry",
    "SymbolIterator": "Symbol.iterator",
    "SymbolAsyncIterator": "Symbol.asyncIterator",
    "SymbolToPrimitive": "Symbol.toPrimitive",
    "SymbolToStringTag": "Symbol.toStringTag",
    "SymbolHasInstance": "Symbol.hasInstance",
    "SymbolDispose": "(Symbol.dispose || Symbol.for('nodejs.dispose'))",
    "SymbolAsyncDispose": "(Symbol.asyncDispose || Symbol.for('nodejs.asyncDispose'))",
    "globalThis": None,  # already global
}


def primordial_def(name):
    """Return the JS expression for a primordial name, or None if unknown."""
    if name in SPECIAL:
        return SPECIAL[name]
    # Bare intrinsic names (Symbol, Boolean, Map, ...) are primordials keys
    # too; map them to the identically-named global.
    if re.fullmatch(GLOBALS, name) and name != "TypedArray":
        return name
    m = re.match(r"^(%s)Prototype([A-Z].*)$" % GLOBALS, name)
    if m:
        g, method = m.group(1), m.group(2)
        method = method[0].lower() + method[1:]
        if g == "TypedArray":
            proto = "Object.getPrototypeOf(Uint8Array).prototype"
        else:
            proto = f"{g}.prototype"
        # Getter-backed props (e.g. TypedArrayPrototypeLength) need care;
        # the modules we build only use methods, so uncurry is enough.
        return f"__uncurry({proto}.{method})"
    m = re.match(r"^(%s)([A-Z].*)$" % GLOBALS, name)
    if m:
        g, member = m.group(1), m.group(2)
        if g == "TypedArray":
            return "Object.getPrototypeOf(Uint8Array)"
        if re.fullmatch(r"[A-Z0-9_]+", member):  # constant: keep case
            return f"{g}.{member}"
        member = member[0].lower() + member[1:]
        return f"{g}.{member}"
    return None


def collect_primordials(source):
    used = {}
    for name in sorted(set(re.findall(r"\b[A-Z][A-Za-z0-9_]{2,}\b", source))):
        d = primordial_def(name)
        if d is not None and name not in ("TypedArray",):
            used[name] = d
    return used


PRELUDE_HELPERS = """\
const __uncurry = (fn) => (thisArg, ...args) => fn.apply(thisArg, args);
"""

# Stub sources for internal modules, shared across generated files. Each is
# a function(require) returning the module's exports.
INTERNAL_STUBS = r"""
const __internalCache = new Map();
function __req(id) {
    if (__internalCache.has(id)) return __internalCache.get(id);
    const factory = __internalModules[id];
    if (!factory) throw new Error('Cannot find module: ' + id);
    const mod = { exports: {} };
    __internalCache.set(id, mod.exports);
    const result = factory(mod, __req);
    if (result !== undefined) {
        __internalCache.set(id, result);
        return result;
    }
    __internalCache.set(id, mod.exports);
    return mod.exports;
}
const __internalModules = {
    'internal/validators': function () {
        const { ERR_INVALID_ARG_TYPE, ERR_OUT_OF_RANGE } = __req('internal/errors').codes;
        function validateString(value, name) {
            if (typeof value !== 'string') throw new ERR_INVALID_ARG_TYPE(name, 'string', value);
        }
        function validateObject(value, name) {
            if (value === null || typeof value !== 'object' || Array.isArray(value)) {
                throw new ERR_INVALID_ARG_TYPE(name, 'object', value);
            }
        }
        function validateFunction(value, name) {
            if (typeof value !== 'function') throw new ERR_INVALID_ARG_TYPE(name, 'function', value);
        }
        function validateBoolean(value, name) {
            if (typeof value !== 'boolean') throw new ERR_INVALID_ARG_TYPE(name, 'boolean', value);
        }
        function validateNumber(value, name, min, max) {
            if (typeof value !== 'number') throw new ERR_INVALID_ARG_TYPE(name, 'number', value);
            if ((min !== undefined && value < min) || (max !== undefined && value > max) ||
                Number.isNaN(value)) {
                throw new ERR_OUT_OF_RANGE(name, `${min !== undefined ? `>= ${min}` : ''}${max !== undefined ? ` && <= ${max}` : ''}`, value);
            }
        }
        function validateInteger(value, name, min = Number.MIN_SAFE_INTEGER, max = Number.MAX_SAFE_INTEGER) {
            if (typeof value !== 'number') throw new ERR_INVALID_ARG_TYPE(name, 'number', value);
            if (!Number.isInteger(value)) throw new ERR_OUT_OF_RANGE(name, 'an integer', value);
            if (value < min || value > max) throw new ERR_OUT_OF_RANGE(name, `>= ${min} && <= ${max}`, value);
        }
        function validateAbortSignal(signal, name) {
            if (signal !== undefined &&
                (signal === null || typeof signal !== 'object' || !('aborted' in signal))) {
                throw new ERR_INVALID_ARG_TYPE(name, 'AbortSignal', signal);
            }
        }
        return {
            validateString, validateObject, validateFunction, validateBoolean,
            validateNumber, validateInteger, validateAbortSignal,
        };
    },
    'internal/errors': function () {
        function determineSpecificType(value) {
            if (value === null) return 'null';
            if (value === undefined) return 'undefined';
            if (typeof value === 'function') return 'function ' + (value.name || '(anonymous)');
            if (typeof value === 'object') {
                return value.constructor && value.constructor.name
                    ? 'an instance of ' + value.constructor.name : 'an object';
            }
            let printed = typeof value === 'string'
                ? "'" + value + "'" : String(value);
            if (printed.length > 28) printed = printed.slice(0, 25) + '...';
            return `type ${typeof value} (${printed})`;
        }
        function makeCode(code, defaultBase, format) {
            const Base = defaultBase;
            class NodeError extends Base {
                constructor(...args) {
                    super(format(...args));
                    this.code = code;
                }
                get ['constructor']() { return Base; }
                toString() { return `${this.name} [${code}]: ${this.message}`; }
            }
            Object.defineProperty(NodeError.prototype, 'name', {
                value: Base.name, writable: true, configurable: true,
            });
            return NodeError;
        }
        const codes = {
            ERR_INVALID_ARG_TYPE: makeCode('ERR_INVALID_ARG_TYPE', TypeError, (name, expected, actual) => {
                const list = Array.isArray(expected) ? expected : [expected];
                const types = list.filter((t) => /^[a-z]/.test(String(t)));
                const instances = list.filter((t) => /^[A-Z]/.test(String(t)));
                const parts = [];
                if (types.length) parts.push(`of type ${types.join(' or ')}`);
                if (instances.length) parts.push(`an instance of ${instances.join(' or ')}`);
                return `The "${name}" argument must be ${parts.join(' or ')}. Received ${determineSpecificType(actual)}`;
            }),
            ERR_INVALID_ARG_VALUE: makeCode('ERR_INVALID_ARG_VALUE', TypeError, (name, value, reason = 'is invalid') => {
                return `The argument '${name}' ${reason}. Received ${determineSpecificType(value)}`;
            }),
            ERR_OUT_OF_RANGE: makeCode('ERR_OUT_OF_RANGE', RangeError, (name, range, actual) => {
                return `The value of "${name}" is out of range. It must be ${range}. Received ${actual}`;
            }),
            ERR_INVALID_THIS: makeCode('ERR_INVALID_THIS', TypeError, (type) => `Value of "this" must be of type ${type}`),
            ERR_UNHANDLED_ERROR: makeCode('ERR_UNHANDLED_ERROR', Error, (err) => {
                return err === undefined ? "Unhandled error." : `Unhandled error. (${err})`;
            }),
            ERR_INVALID_URI: makeCode('ERR_INVALID_URI', URIError, () => 'URI malformed'),
        };
        class AbortError extends Error {
            constructor(message = 'The operation was aborted', options = undefined) {
                super(message, options);
                this.code = 'ABORT_ERR';
                this.name = 'AbortError';
            }
        }
        function genericNodeError(message, options) {
            const err = new Error(message);
            if (options) Object.assign(err, options);
            return err;
        }
        return {
            codes, AbortError, genericNodeError,
            kEnhanceStackBeforeInspector: Symbol('kEnhanceStackBeforeInspector'),
        };
    },
    'internal/util': function () {
        return {
            getLazy(init) {
                let called = false, value;
                return () => {
                    if (!called) { called = true; value = init(); }
                    return value;
                };
            },
            emitExperimentalWarning() {},
            isWindows: false,
            isMacOS: false,
            SymbolDispose: Symbol.dispose || Symbol.for('nodejs.dispose'),
            kEmptyObject: Object.freeze({ __proto__: null }),
            spliceOne(list, index) {
                for (; index + 1 < list.length; index++) list[index] = list[index + 1];
                list.pop();
            },
        };
    },
    'internal/util/inspect': function () {
        return {
            inspect: function inspect(value) {
                // A throwing [util.inspect.custom] propagates (callers like
                // events.js catch it and fall back to String coercion).
                if (value !== null && typeof value === 'object') {
                    const custom = value[Symbol.for('nodejs.util.inspect.custom')];
                    if (typeof custom === 'function') {
                        const res = custom.call(value, 2, {});
                        return typeof res === 'string' ? res : inspect(res);
                    }
                }
                try {
                    if (typeof value === 'string') {
                        return "'" + value.replace(/\\/g, '\\\\').replace(/'/g, "\\'") + "'";
                    }
                    if (value === null || typeof value !== 'object') return String(value);
                    if (value instanceof Error) return value.stack || String(value);
                    if (Array.isArray(value)) {
                        return '[ ' + value.map(inspect).join(', ') + ' ]';
                    }
                    const props = Object.keys(value).map((k) => k + ': ' + inspect(value[k]));
                    return props.length ? '{ ' + props.join(', ') + ' }' : '{}';
                } catch { return String(value); }
            },
            identicalSequenceRange() { return { len: 0, offset: 0 }; },
        };
    },
    'internal/events/abort_listener': function () {
        const { SymbolDispose } = __req('internal/util');
        return {
            addAbortListener(signal, listener) {
                if (signal.aborted) {
                    queueMicrotask(() => listener());
                } else {
                    signal.addEventListener('abort', listener, { once: true });
                }
                return {
                    [SymbolDispose]() {
                        try { signal.removeEventListener('abort', listener); } catch {}
                    },
                };
            },
        };
    },
    'internal/event_target': function () {
        return {
            isEventTarget(value) {
                return value != null && typeof value.addEventListener === 'function' &&
                    typeof value.dispatchEvent === 'function' &&
                    typeof value.emit !== 'function';
            },
            kEvents: Symbol('kEvents'),
            kResistStopPropagation: Symbol('kResistStopPropagation'),
        };
    },
    'internal/events/symbols': function () {
        return { kFirstEventParam: Symbol('kFirstEventParam') };
    },
    'async_hooks': function () {
        return {
            AsyncResource: class AsyncResource {
                constructor() {}
                runInAsyncScope(fn, thisArg, ...args) { return fn.apply(thisArg, args); }
                emitDestroy() { return this; }
            },
        };
    },
    'internal/deps/minimatch/index': function () {
        throw new Error('path.matchesGlob is not supported in this runtime');
    },
};
"""


def wrap(name, node_sources, extra_internal, header_imports, exports_js, process_needed):
    """Build one generated ESM module."""
    combined_scan = "".join(s for _, s in node_sources) + extra_internal
    prims = collect_primordials(combined_scan)
    prim_lines = (
        "const primordials = {\n"
        + "\n".join(f"  {n}: {d}," for n, d in prims.items())
        + "\n};"
    )

    embedded = []
    for mod_id, src in node_sources[1:]:
        embedded.append(
            "__internalModules[%s] = function (module, require) {\n"
            "'use strict';\n%s\nreturn module.exports;\n};\n" % (json.dumps(mod_id), src)
        )

    main_src = node_sources[0][1]
    process_shim = (
        "const process = __nodeProcess;\n" if process_needed else ""
    )

    return f"""\
// GENERATED by tools/compat/gen-node-modules.py — do not edit by hand.
// Node.js {VERSION} lib sources (MIT, (c) Node.js contributors) running
// verbatim against a primordials shim; see the generator for the stubs.
{header_imports}
{PRELUDE_HELPERS}
{prim_lines}
{INTERNAL_STUBS}
{extra_internal}
{"".join(embedded)}
{process_shim}
const __module = {{ exports: {{}} }};
(function (module, exports, require) {{
'use strict';
{main_src}
}})(__module, __module.exports, __req);
const __exports = __module.exports;
{exports_js}
"""


def strip_module_header(src):
    # Node lib files start with a license comment then 'use strict';
    # keep everything, it runs inside our wrapper fine.
    return src


path_src = (SRC / "lib/path.js").read_text()
constants_src = (SRC / "lib/internal/constants.js").read_text()
qs_src = (SRC / "lib/querystring.js").read_text()
qs_internal_src = (SRC / "lib/internal/querystring.js").read_text()
events_src = (SRC / "lib/events.js").read_text()
fixed_queue_src = (SRC / "lib/internal/fixed_queue.js").read_text()

# ── path ────────────────────────────────────────────────────────────────
constants_embed = (
    "__internalModules['internal/constants'] = function (module, require) {\n"
    "'use strict';\n" + constants_src + "\nreturn module.exports;\n};\n"
)
(OUT / "path.js").write_text(wrap(
    "path",
    [("path", path_src)],
    constants_embed,
    "import __nodeProcess from 'node:process';",
    """\
export default __exports;
export const {
  win32, posix, basename, dirname, extname, format, isAbsolute, join,
  normalize, parse, relative, resolve, sep, delimiter, toNamespacedPath,
} = __exports;
export const matchesGlob = __exports.matchesGlob;
""",
    True,
))

# ── querystring ─────────────────────────────────────────────────────────
qs_internal_embed = (
    "__internalModules['internal/querystring'] = function (module, require) {\n"
    "'use strict';\n" + qs_internal_src + "\nreturn module.exports;\n};\n"
    "__internalModules['buffer'] = function () { return { Buffer: __nodeBuffer }; };\n"
)
(OUT / "querystring.js").write_text(wrap(
    "querystring",
    [("querystring", qs_src)],
    qs_internal_embed,
    "import { Buffer as __nodeBuffer } from 'node:buffer';",
    """\
export default __exports;
export const {
  decode, encode, escape, parse, stringify, unescape, unescapeBuffer,
} = __exports;
""",
    False,
))

# ── events ──────────────────────────────────────────────────────────────
fixed_queue_embed = (
    "__internalModules['internal/fixed_queue'] = function (module, require) {\n"
    "'use strict';\n" + fixed_queue_src + "\nreturn module.exports;\n};\n"
)
(OUT / "events.js").write_text(wrap(
    "events",
    [("events", events_src)],
    fixed_queue_embed,
    "import __nodeProcess from 'node:process';",
    """\
export default __exports;
export const EventEmitter = __exports.EventEmitter || __exports;
export const {
  once, on, getEventListeners, getMaxListeners, setMaxListeners,
  captureRejectionSymbol, errorMonitor, addAbortListener,
  EventEmitterAsyncResource, usingDomains,
} = __exports;
export const defaultMaxListeners = __exports.defaultMaxListeners;
""",
    True,
))

print(f"generated path/querystring/events from Node {VERSION} into {OUT}")
