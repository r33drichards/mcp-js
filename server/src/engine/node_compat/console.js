// node:console — the runtime's console global under Node's module name,
// plus a Console class shaped by nodejs/node's own
// test-console-instance.js (vendored into the node_compat suite): a
// function-style constructor (callable without `new`), prototype methods
// re-bound as own properties in the constructor (so `const { log } = c`
// works and subclass overrides still win), `ignoreErrors` swallowing
// stream write errors, and brand-based instanceof that also claims the
// global console. Inspector integration and color mode are not provided.

import { format, inspect } from 'node:util';

const globalConsole = globalThis.console;

const kBrand = Symbol('mcp-v8.console.instance');
const kStdout = Symbol('mcp-v8.console.stdout');
const kStderr = Symbol('mcp-v8.console.stderr');
const kIgnoreErrors = Symbol('mcp-v8.console.ignoreErrors');

// Mirrors Node's ERR_INVALID_ARG_TYPE "Received ..." suffix, which
// test-console-instance.js checks verbatim for inspectOptions.
function received(input) {
    if (input == null) return ` Received ${input}`;
    if (typeof input === 'function') {
        return ` Received function ${input.name || '(anonymous)'}`;
    }
    if (typeof input === 'object') {
        if (input.constructor && input.constructor.name) {
            return ` Received an instance of ${input.constructor.name}`;
        }
        return ` Received ${inspect(input, { depth: -1 })}`;
    }
    let inspected = inspect(input, { colors: false });
    if (inspected.length > 28) inspected = inspected.slice(0, 25) + '...';
    return ` Received type ${typeof input} (${inspected})`;
}

function streamError(name) {
    const err = new TypeError(
        `Console expects a writable stream instance for ${name}`);
    err.code = 'ERR_CONSOLE_WRITABLE_STREAM';
    return err;
}

export function Console(options, stderr, ignoreErrors = true) {
    // Callable without `new` (instanceof can't distinguish here: the brand
    // that drives it is only set further down in construction).
    if (new.target === undefined) {
        return new Console(options, stderr, ignoreErrors);
    }

    let stdout = options;
    if (options !== null && typeof options === 'object' &&
        typeof options.write !== 'function' &&
        ('stdout' in options || 'stderr' in options)) {
        stdout = options.stdout;
        stderr = options.stderr;
        if (options.ignoreErrors !== undefined) ignoreErrors = options.ignoreErrors;
        const inspectOptions = options.inspectOptions;
        if (inspectOptions !== undefined &&
            (typeof inspectOptions !== 'object' || inspectOptions === null)) {
            const err = new TypeError(
                'The "options.inspectOptions" property must be of type object.' +
                received(inspectOptions));
            err.code = 'ERR_INVALID_ARG_TYPE';
            throw err;
        }
    }

    if (!stdout || typeof stdout.write !== 'function') throw streamError('stdout');
    if (stderr === undefined || stderr === null) {
        stderr = stdout;
    } else if (typeof stderr.write !== 'function') {
        throw streamError('stderr');
    }

    this[kBrand] = true;
    this[kStdout] = stdout;
    this[kStderr] = stderr;
    this[kIgnoreErrors] = ignoreErrors !== false;

    // Bind through `this` so a subclass's prototype override is what gets
    // bound (test: `class MyConsole extends Console { log() {...} }`).
    for (const key of METHOD_NAMES) {
        this[key] = this[key].bind(this);
    }
}

function writeLine(self, streamKey, text) {
    try {
        self[streamKey].write(text + '\n');
    } catch (err) {
        if (!self[kIgnoreErrors]) throw err;
    }
}

Console.prototype.log = function log(...args) {
    writeLine(this, kStdout, format(...args));
};
Console.prototype.info = function info(...args) {
    writeLine(this, kStdout, format(...args));
};
Console.prototype.debug = function debug(...args) {
    writeLine(this, kStdout, format(...args));
};
Console.prototype.dir = function dir(obj, options) {
    writeLine(this, kStdout, inspect(obj, options));
};
Console.prototype.warn = function warn(...args) {
    writeLine(this, kStderr, format(...args));
};
Console.prototype.error = function error(...args) {
    writeLine(this, kStderr, format(...args));
};
Console.prototype.trace = function trace(...args) {
    const err = new Error(format(...args));
    err.name = 'Trace';
    writeLine(this, kStderr, err.stack || String(err));
};
Console.prototype.assert = function assert(condition, ...args) {
    if (condition) return;
    if (args.length > 0 && typeof args[0] === 'string') {
        args[0] = 'Assertion failed: ' + args[0];
    } else {
        args.unshift('Assertion failed');
    }
    this.warn(...args);
};

const METHOD_NAMES = Object.freeze([
    'log', 'info', 'debug', 'dir', 'warn', 'error', 'trace', 'assert',
]);

// Brand-based instanceof: `globalThis.console instanceof Console` holds
// (Node marks its global console the same way) while `{} instanceof
// Console` does not.
Object.defineProperty(Console, Symbol.hasInstance, {
    value: function hasInstance(instance) {
        return instance !== null && instance !== undefined &&
            instance[kBrand] === true;
    },
});
Object.defineProperty(globalConsole, kBrand, {
    value: true, writable: false, enumerable: false, configurable: true,
});

// Node's `require('console')` is the global console object, with the
// class reachable as `console.Console`.
if (typeof globalConsole.Console !== 'function') {
    try {
        Object.defineProperty(globalConsole, 'Console', {
            value: Console, writable: true, enumerable: false, configurable: true,
        });
    } catch (_) { /* frozen console: module exports still carry the class */ }
}

export default globalConsole;
export const {
    log, info, warn, error, debug, trace, dir, dirxml, table, clear,
    group, groupCollapsed, groupEnd, count, countReset,
    assert, time, timeLog, timeEnd,
} = globalConsole;
