// node:assert — the strict-comparison core used by Node's own test suite:
// ok/equal/strictEqual/deepStrictEqual and friends, throws/rejects with
// error validation (constructor, regexp, object, function), match, fail,
// AssertionError. Legacy-loose deepEqual/equal are supported with
// SameValueZero-ish coercion semantics close enough for the vendored
// tests.
import { inspect } from 'node:util';

function codedTypeError(code, message) {
    const error = new TypeError(message);
    error.code = code;
    return error;
}

function formatReceived(value) {
    if (value === undefined || value === null) return String(value);
    if (typeof value === 'object') {
        const name = value.constructor && value.constructor.name;
        return name ? 'an instance of ' + name : inspect(value);
    }
    return 'type ' + typeof value + ' (' + inspect(value) + ')';
}

function isPromise(value) {
    return value !== null && typeof value === 'object' &&
        typeof value.then === 'function' && typeof value.catch === 'function';
}

function promiseFrom(promiseOrFn) {
    if (typeof promiseOrFn === 'function') {
        const promise = promiseOrFn();
        if (!isPromise(promise)) {
            const received = promise === undefined || promise === null
                ? String(promise)
                : typeof promise === 'object' && promise.constructor && promise.constructor.name
                    ? 'an instance of ' + promise.constructor.name
                    : typeof promise;
            throw codedTypeError('ERR_INVALID_RETURN_VALUE',
                'Expected instance of Promise to be returned from the "promiseFn" ' +
                'function but got ' + received + '.');
        }
        return promise;
    }
    if (!isPromise(promiseOrFn)) {
        throw codedTypeError('ERR_INVALID_ARG_TYPE',
            'The "promiseFn" argument must be of type function or an instance of Promise. Received ' +
            formatReceived(promiseOrFn));
    }
    return promiseOrFn;
}

class AssertionError extends Error {
    constructor(options) {
        const {
            message, actual, expected, operator, stackStartFn, generatedMessage,
        } = options || {};
        let msg = message;
        if (msg == null) {
            msg = `${inspect(actual, { depth: 3 })} ${operator} ${inspect(expected, { depth: 3 })}`;
        }
        super(String(msg));
        this.name = 'AssertionError';
        this.code = 'ERR_ASSERTION';
        this.actual = actual;
        this.expected = expected;
        this.operator = operator;
        this.generatedMessage = generatedMessage ?? message == null;
        if (Error.captureStackTrace) {
            Error.captureStackTrace(this, stackStartFn || AssertionError);
        }
    }
}

function isDeepEqual(a, b, strict, memo) {
    if (Object.is(a, b)) return true;
    if (!strict && a == null && b == null) return a === b || (a == b); // eslint-disable-line eqeqeq
    if (typeof a === 'number' && typeof b === 'number') {
        return strict ? Object.is(a, b) : (a === b || (Number.isNaN(a) && Number.isNaN(b)));
    }
    if (strict) {
        if (typeof a !== typeof b) return false;
        if (typeof a !== 'object' || a === null || b === null) return a === b;
        if (Object.getPrototypeOf(a) !== Object.getPrototypeOf(b)) return false;
    } else {
        if (a == null || b == null) return a == b; // eslint-disable-line eqeqeq
        if (typeof a !== 'object' && typeof b !== 'object') return a == b; // eslint-disable-line eqeqeq
        if (typeof a !== 'object' || typeof b !== 'object') return false;
    }

    memo = memo || new Map();
    const prior = memo.get(a);
    if (prior !== undefined && prior === b) return true;
    memo.set(a, b);

    if (Array.isArray(a)) {
        if (!Array.isArray(b) || a.length !== b.length) return false;
    }
    if (a instanceof Date) {
        return b instanceof Date && a.getTime() === b.getTime();
    }
    if (a instanceof RegExp) {
        return b instanceof RegExp && a.source === b.source && a.flags === b.flags;
    }
    if (ArrayBuffer.isView(a) && !(a instanceof DataView)) {
        if (!ArrayBuffer.isView(b) || a.constructor !== b.constructor || a.length !== b.length) {
            return false;
        }
        for (let i = 0; i < a.length; i++) {
            if (!Object.is(a[i], b[i])) return false;
        }
        return true;
    }
    if (a instanceof ArrayBuffer) {
        if (!(b instanceof ArrayBuffer) || a.byteLength !== b.byteLength) return false;
        const va = new Uint8Array(a), vb = new Uint8Array(b);
        for (let i = 0; i < va.length; i++) if (va[i] !== vb[i]) return false;
        return true;
    }
    if (a instanceof Map) {
        if (!(b instanceof Map) || a.size !== b.size) return false;
        for (const [k, v] of a) {
            if (!b.has(k)) {
                // Non-reference keys need a deep scan.
                let found = false;
                for (const [bk, bv] of b) {
                    if (isDeepEqual(k, bk, strict, memo) && isDeepEqual(v, bv, strict, memo)) {
                        found = true;
                        break;
                    }
                }
                if (!found) return false;
            } else if (!isDeepEqual(v, b.get(k), strict, memo)) {
                return false;
            }
        }
        return true;
    }
    if (a instanceof Set) {
        if (!(b instanceof Set) || a.size !== b.size) return false;
        for (const v of a) {
            if (!b.has(v)) {
                let found = false;
                for (const bv of b) {
                    if (isDeepEqual(v, bv, strict, memo)) { found = true; break; }
                }
                if (!found) return false;
            }
        }
        return true;
    }
    if (a instanceof Error) {
        if (!(b instanceof Error)) return false;
        if (a.message !== b.message || a.name !== b.name) return false;
    }
    const boxed = Object.prototype.toString.call(a);
    if (['[object Boolean]', '[object Number]', '[object String]', '[object BigInt]'].includes(boxed)) {
        if (Object.prototype.toString.call(b) !== boxed) return false;
        if (!Object.is(a.valueOf(), b.valueOf())) return false;
    }

    const keysA = Object.keys(a);
    const keysB = Object.keys(b);
    if (keysA.length !== keysB.length) return false;
    for (const k of keysA) {
        if (!Object.prototype.hasOwnProperty.call(b, k)) return false;
        if (!isDeepEqual(a[k], b[k], strict, memo)) return false;
    }
    const symsA = Object.getOwnPropertySymbols(a).filter((s) =>
        Object.getOwnPropertyDescriptor(a, s).enumerable);
    const symsB = Object.getOwnPropertySymbols(b).filter((s) =>
        Object.getOwnPropertyDescriptor(b, s).enumerable);
    if (symsA.length !== symsB.length) return false;
    for (const s of symsA) {
        if (!Object.prototype.hasOwnProperty.call(b, s)) return false;
        if (!isDeepEqual(a[s], b[s], strict, memo)) return false;
    }
    return true;
}

function innerFail(obj) {
    throw new AssertionError(obj);
}

function ok(value, message) {
    if (!value) {
        if (message instanceof Error) throw message;
        innerFail({
            actual: value, expected: true,
            message: message ?? `The expression evaluated to a falsy value: assert.ok(${inspect(value)})`,
            operator: '==', stackStartFn: ok,
        });
    }
}

function checkExpected(actual, expected, message, fn, operator) {
    if (typeof expected === 'function') {
        if (expected.prototype !== undefined && actual instanceof expected) return true;
        if (Object.getPrototypeOf(expected) === Error || expected === Error ||
            Error.isPrototypeOf?.(expected)) {
            return false;
        }
        // Validation function.
        return expected.call({}, actual) === true;
    }
    if (expected instanceof RegExp) {
        // Node tests the stringified error ("Error: msg"), not the message
        // alone — anchored patterns like /^Error: out$/ depend on it.
        return expected.test(String(actual));
    }
    if (typeof expected === 'object' && expected !== null) {
        if (actual === null || (typeof actual !== 'object' && typeof actual !== 'function')) {
            return false;
        }
        for (const k of [...Object.keys(expected), ...Object.getOwnPropertySymbols(expected)]) {
            const ev = expected[k];
            const av = actual[k];
            if (ev instanceof RegExp) {
                if (!ev.test(String(av))) return false;
            } else if (!isDeepEqual(av, ev, true)) {
                return false;
            }
        }
        return true;
    }
    return undefined;
}

function validateThrown(thrown, expected, message, fn, operator) {
    if (expected === undefined) return;
    if (typeof expected === 'string') {
        // string expected means it's actually the message parameter
        return;
    }
    if (typeof expected === 'function' && expected.prototype !== undefined &&
        (expected === Error || Error.prototype.isPrototypeOf(expected.prototype))) {
        if (!(thrown instanceof expected)) {
            innerFail({
                actual: thrown, expected, operator,
                message: message ?? `The error is expected to be an instance of "${expected.name}". Received ${inspect(thrown)}`,
                stackStartFn: fn,
            });
        }
        return;
    }
    if (typeof expected === 'function') {
        const result = expected.call({}, thrown);
        if (result !== true) {
            innerFail({
                actual: thrown, expected, operator,
                message: message ??
                    `The "validate" validation function is expected to return "true". ` +
                    `Received ${inspect(result)}\n\nCaught error:\n\n${String(thrown)}`,
                stackStartFn: fn,
            });
        }
        return;
    }
    const result = checkExpected(thrown, expected);
    if (result === false) {
        innerFail({
            actual: thrown, expected, operator,
            message: message ?? `The error does not match the expected validation: ${inspect(expected)}. Received ${inspect(thrown)}`,
            generatedMessage: message == null,
            stackStartFn: fn,
        });
    }
}

function throws(fn, expected, message) {
    if (typeof expected === 'string') { message = expected; expected = undefined; }
    let thrown = null, didThrow = false;
    try {
        fn();
    } catch (e) {
        thrown = e;
        didThrow = true;
    }
    if (!didThrow) {
        innerFail({
            actual: undefined, expected, operator: 'throws',
            message: message ?? 'Missing expected exception.',
            stackStartFn: throws,
        });
    }
    validateThrown(thrown, expected, message, throws, 'throws');
    return thrown;
}

async function rejects(promiseOrFn, expected, message) {
    if (typeof expected === 'string') { message = expected; expected = undefined; }
    const promise = promiseFrom(promiseOrFn);
    let thrown = null, didReject = false;
    try {
        await promise;
    } catch (e) {
        thrown = e;
        didReject = true;
    }
    if (!didReject) {
        innerFail({
            actual: undefined, expected, operator: 'rejects',
            message: message ?? `Missing expected rejection${
                typeof expected === 'function' && expected.name ? ` (${expected.name})` : ''}.`,
            stackStartFn: rejects,
        });
    }
    validateThrown(thrown, expected, message, rejects, 'rejects');
}

function doesNotThrow(fn, expected, message) {
    if (typeof expected === 'string') { message = expected; expected = undefined; }
    try {
        fn();
    } catch (e) {
        if (expected === undefined || checkExpected(e, expected) !== false) {
            innerFail({
                actual: e, expected, operator: 'doesNotThrow',
                message: message ?? `Got unwanted exception.\nActual message: "${e && e.message}"`,
                stackStartFn: doesNotThrow,
            });
        }
        throw e;
    }
}

async function doesNotReject(promiseOrFn, expected, message) {
    if (typeof expected === 'string') { message = expected; expected = undefined; }
    const promise = promiseFrom(promiseOrFn);
    try {
        await promise;
    } catch (e) {
        if (expected === undefined || checkExpected(e, expected) !== false) {
            innerFail({
                actual: e, expected, operator: 'doesNotReject',
                message: message ?? `Got unwanted rejection.\nActual message: "${e && e.message}"`,
                stackStartFn: doesNotReject,
            });
        }
        throw e;
    }
}

const assert = Object.assign(ok, {
    AssertionError,
    ok,
    fail(message) {
        if (message instanceof Error) throw message;
        innerFail({
            actual: undefined, expected: undefined, operator: 'fail',
            message: message ?? 'Failed', stackStartFn: assert.fail,
        });
    },
    equal(actual, expected, message) {
        // eslint-disable-next-line eqeqeq
        if (actual != expected && !(Number.isNaN(actual) && Number.isNaN(expected))) {
            innerFail({ actual, expected, operator: '==', message, stackStartFn: assert.equal });
        }
    },
    notEqual(actual, expected, message) {
        // eslint-disable-next-line eqeqeq
        if (actual == expected || (Number.isNaN(actual) && Number.isNaN(expected))) {
            innerFail({ actual, expected, operator: '!=', message, stackStartFn: assert.notEqual });
        }
    },
    strictEqual(actual, expected, message) {
        if (!Object.is(actual, expected)) {
            innerFail({ actual, expected, operator: 'strictEqual', message, stackStartFn: assert.strictEqual });
        }
    },
    notStrictEqual(actual, expected, message) {
        if (Object.is(actual, expected)) {
            innerFail({ actual, expected, operator: 'notStrictEqual', message, stackStartFn: assert.notStrictEqual });
        }
    },
    deepEqual(actual, expected, message) {
        if (!isDeepEqual(actual, expected, false)) {
            innerFail({ actual, expected, operator: 'deepEqual', message, stackStartFn: assert.deepEqual });
        }
    },
    notDeepEqual(actual, expected, message) {
        if (isDeepEqual(actual, expected, false)) {
            innerFail({ actual, expected, operator: 'notDeepEqual', message, stackStartFn: assert.notDeepEqual });
        }
    },
    deepStrictEqual(actual, expected, message) {
        if (!isDeepEqual(actual, expected, true)) {
            innerFail({ actual, expected, operator: 'deepStrictEqual', message, stackStartFn: assert.deepStrictEqual });
        }
    },
    notDeepStrictEqual(actual, expected, message) {
        if (isDeepEqual(actual, expected, true)) {
            innerFail({ actual, expected, operator: 'notDeepStrictEqual', message, stackStartFn: assert.notDeepStrictEqual });
        }
    },
    match(string, regexp, message) {
        if (!(regexp instanceof RegExp)) {
            throw new TypeError('The "regexp" argument must be an instance of RegExp');
        }
        if (typeof string !== 'string' || !regexp.test(string)) {
            innerFail({
                actual: string, expected: regexp, operator: 'match',
                message: message ?? `The input did not match the regular expression ${regexp}. Input:\n\n${inspect(string)}\n`,
                stackStartFn: assert.match,
            });
        }
    },
    doesNotMatch(string, regexp, message) {
        if (!(regexp instanceof RegExp)) {
            throw new TypeError('The "regexp" argument must be an instance of RegExp');
        }
        if (typeof string === 'string' && regexp.test(string)) {
            innerFail({
                actual: string, expected: regexp, operator: 'doesNotMatch',
                message: message ?? `The input was expected to not match the regular expression ${regexp}. Input:\n\n${inspect(string)}\n`,
                stackStartFn: assert.doesNotMatch,
            });
        }
    },
    throws,
    rejects,
    doesNotThrow,
    doesNotReject,
    ifError(value) {
        if (value !== null && value !== undefined) {
            innerFail({
                actual: value, expected: null, operator: 'ifError',
                message: `ifError got unwanted exception: ${value instanceof Error ? value.message : inspect(value)}`,
                stackStartFn: assert.ifError,
            });
        }
    },
});

// assert.strict mirrors assert with strict semantics for the loose methods.
const strict = Object.assign(function strictOk(...args) { return ok(...args); }, assert, {
    equal: assert.strictEqual,
    notEqual: assert.notStrictEqual,
    deepEqual: assert.deepStrictEqual,
    notDeepEqual: assert.notDeepStrictEqual,
});
strict.strict = strict;
assert.strict = strict;

export default assert;
export {
    AssertionError, ok, throws, rejects, doesNotThrow, doesNotReject, strict,
};
export const {
    fail, equal, notEqual, strictEqual, notStrictEqual, deepEqual,
    notDeepEqual, deepStrictEqual, notDeepStrictEqual, match, doesNotMatch,
    ifError,
} = assert;
