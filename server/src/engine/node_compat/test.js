// node:test - sequential test and suite execution for the isolate runtime.
// The API preserves Node's registration model, nested hooks, async callbacks,
// and promise return values. Concurrency options are accepted but execution is
// intentionally serialized because one isolate owns one event loop.

function deferred() {
    let resolve;
    let reject;
    const promise = new Promise((res, rej) => { resolve = res; reject = rej; });
    // Node tests commonly ignore the returned thenable; the runner owns
    // failure reporting, so attach a handler without changing await behavior.
    promise.catch(() => {});
    return { promise, resolve, reject };
}

function normalizeArgs(name, options, fn) {
    if (typeof name === 'function') return { name: name.name || '<anonymous>', options: {}, fn: name };
    if (typeof options === 'function') return { name: String(name), options: {}, fn: options };
    return { name: String(name), options: options || {}, fn: fn || (() => {}) };
}

function recordFailure(error) {
    const record = globalThis.__NODE_TEST_RECORD_FAILURE__;
    if (typeof record === 'function') record(error);
}

function track(promise) {
    globalThis.__NODE_TEST_PENDING__ = (globalThis.__NODE_TEST_PENDING__ || 0) + 1;
    promise.catch(recordFailure).finally(() => {
        globalThis.__NODE_TEST_PENDING__--;
    });
    return promise;
}

function makeSuite(name, options, parent) {
    return {
        name, options, parent, children: [],
        hooks: { before: [], after: [], beforeEach: [], afterEach: [] },
        completion: deferred(),
    };
}

const root = makeSuite('<root>', {}, null);
let currentSuite = null;

function hook(kind, fn) {
    if (typeof fn !== 'function') throw new TypeError(`${kind} hook must be a function`);
    (currentSuite || root).hooks[kind].push(fn);
}

async function callHook(fn, context) {
    await fn(context);
}

function inheritedHooks(suite, kind) {
    const chain = [];
    for (let cursor = suite; cursor && cursor !== root; cursor = cursor.parent) {
        chain.push(cursor);
    }
    if (kind === 'beforeEach') chain.reverse();
    return chain.flatMap((item) => item.hooks[kind]);
}

function testContext(node) {
    return {
        name: node.name,
        signal: new AbortController().signal,
        diagnostic(message) { console.log(String(message)); },
        skip() { node.options.skip = true; },
        todo() { node.options.todo = true; },
        test: (name, options, fn) => registerTest(node.parent, name, options, fn),
        before: (fn) => node.parent.hooks.before.push(fn),
        after: (fn) => node.parent.hooks.after.push(fn),
        beforeEach: (fn) => node.parent.hooks.beforeEach.push(fn),
        afterEach: (fn) => node.parent.hooks.afterEach.push(fn),
    };
}

async function runTest(node) {
    if (node.options.skip || node.options.todo) return;
    const context = testContext(node);
    const beforeEachHooks = inheritedHooks(node.parent, 'beforeEach');
    const afterEachHooks = inheritedHooks(node.parent, 'afterEach').reverse();
    for (const fn of beforeEachHooks) await callHook(fn, context);
    try {
        await node.fn(context);
    } finally {
        for (const fn of afterEachHooks) await callHook(fn, context);
    }
}

async function runSuite(suite) {
    if (suite.options.skip || suite.options.todo) return;
    const context = { name: suite.name, signal: new AbortController().signal };
    for (const fn of suite.hooks.before) await callHook(fn, context);
    try {
        for (const child of suite.children) {
            if (child.kind === 'suite') await executeSuite(child.value);
            else await executeTest(child.value);
        }
    } finally {
        for (const fn of suite.hooks.after) await callHook(fn, context);
    }
}

async function executeSuite(suite) {
    try {
        await runSuite(suite);
        suite.completion.resolve();
    } catch (error) {
        suite.completion.reject(error);
        throw error;
    }
}

async function executeTest(node) {
    try {
        await runTest(node);
        node.completion.resolve();
    } catch (error) {
        node.completion.reject(error);
        throw error;
    }
}

function registerTest(parent, name, options, fn) {
    const args = normalizeArgs(name, options, fn);
    const node = { kind: 'test', parent, ...args, completion: deferred() };
    parent.children.push({ kind: 'test', value: node });
    return node.completion.promise;
}

export function describe(name, options, fn) {
    const args = normalizeArgs(name, options, fn);
    const parent = currentSuite || root;
    const suite = makeSuite(args.name, args.options, parent);
    parent.children.push({ kind: 'suite', value: suite });
    const previous = currentSuite;
    currentSuite = suite;
    try {
        args.fn({ name: suite.name, signal: new AbortController().signal });
    } catch (error) {
        suite.completion.reject(error);
        recordFailure(error);
        return suite.completion.promise;
    } finally {
        currentSuite = previous;
    }
    if (parent === root) track(executeSuite(suite));
    return suite.completion.promise;
}

export function test(name, options, fn) {
    const parent = currentSuite || root;
    const promise = registerTest(parent, name, options, fn);
    if (parent === root) {
        const child = parent.children[parent.children.length - 1].value;
        track(executeTest(child));
    }
    return promise;
}

export const suite = describe;
export const it = test;
export const before = (fn) => hook('before', fn);
export const after = (fn) => hook('after', fn);
export const beforeEach = (fn) => hook('beforeEach', fn);
export const afterEach = (fn) => hook('afterEach', fn);
export const only = (name, options, fn) => test(name, { ...(typeof options === 'object' ? options : {}), only: true }, typeof options === 'function' ? options : fn);
export const skip = (name, options, fn) => test(name, { ...(typeof options === 'object' ? options : {}), skip: true }, typeof options === 'function' ? options : fn);
export const todo = (name, options, fn) => test(name, { ...(typeof options === 'object' ? options : {}), todo: true }, typeof options === 'function' ? options : fn);

test.test = test;
test.describe = describe;
test.suite = suite;
test.it = it;
test.before = before;
test.after = after;
test.beforeEach = beforeEach;
test.afterEach = afterEach;
test.only = only;
test.skip = skip;
test.todo = todo;

export default test;
