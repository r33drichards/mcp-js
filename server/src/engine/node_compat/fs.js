// node:fs — import-compatible surface backed by the policy-gated `fs`
// capability when enabled. Without that capability, operations retain the
// rejecting stub behavior used by the default sandbox.

import { Buffer } from 'node:buffer';
import path from 'node:path';

function unsupported(name) {
    return function () {
        const maybeCb = arguments[arguments.length - 1];
        const err = new Error(
            `fs.${name} is not supported in this runtime; use the policy-gated ` +
            'fs capability (globalThis.fs) instead');
        err.code = 'ENOSYS';
        if (typeof maybeCb === 'function') {
            maybeCb(err);
            return;
        }
        throw err;
    };
}

function hostFs(name) {
    const method = globalThis.fs && globalThis.fs[name];
    return typeof method === 'function' ? method.bind(globalThis.fs) : null;
}

// Wrap a host promise method as Node's callback style: trailing callback,
// options argument optional.
function callbackify(name, promiseMethod, pathCount = 1) {
    if (!promiseMethod) return unsupported(name);
    return function (...args) {
        const callback = args.pop();
        if (typeof callback !== 'function') {
            throw new TypeError(`fs.${name}: callback must be a function`);
        }
        const normalized = args.map(
            (value, index) => (index < pathCount ? normalizePath(value) : value));
        promiseMethod(...normalized).then(
            (result) => callback(null, result),
            (error) => callback(error),
        );
    };
}

const hostReadFileSync = hostFs('readFileSync');
export const readFileSync = hostReadFileSync
    ? (path, options) => {
        const encoding = typeof options === 'string'
            ? options
            : options && options.encoding;
        const bytes = hostReadFileSync(normalizePath(path));
        if (encoding && encoding !== 'buffer') {
            return Buffer.from(bytes).toString(encoding);
        }
        return Buffer.from(bytes);
    }
    : unsupported('readFileSync');

export const writeFileSync = globalThis.fs && typeof globalThis.fs.writeFileSync === 'function'
    ? (path, data, ...args) => globalThis.fs.writeFileSync(normalizePath(path), data, ...args)
    : unsupported('writeFileSync');
export const symlinkSync = globalThis.fs && typeof globalThis.fs.symlinkSync === 'function'
    ? (target, path, ...args) => globalThis.fs.symlinkSync(normalizePath(target), normalizePath(path), ...args)
    : unsupported('symlinkSync');

function syncDelegate(name, pathCount = 1) {
    const method = hostFs(name);
    if (!method) return unsupported(name);
    return (...args) => method(
        ...args.map((value, index) => (index < pathCount ? normalizePath(value) : value)));
}

export const mkdirSync = syncDelegate('mkdirSync');
export const statSync = syncDelegate('statSync');
export const lstatSync = syncDelegate('lstatSync');
export const readdirSync = syncDelegate('readdirSync');
export const rmSync = syncDelegate('rmSync');
export const rmdirSync = syncDelegate('rmdirSync');
export const unlinkSync = syncDelegate('unlinkSync');
export const renameSync = syncDelegate('renameSync', 2);
export const copyFileSync = syncDelegate('copyFileSync', 2);
export const readlinkSync = syncDelegate('readlinkSync');

const hostExistsSync = hostFs('existsSync');
export const existsSync = hostExistsSync
    ? (path) => hostExistsSync(normalizePath(path))
    : () => false;

// Recursive copy built on the sync surface. Subset semantics: recursive,
// errorOnExist, and symlink preservation; filters and mode flags are not
// implemented.
function cpSyncImpl(src, dest, options) {
    const stats = lstatSync(src);
    if (stats.isDirectory()) {
        if (!options.recursive) {
            const err = new Error(
                `Recursive option not enabled, error about ${src}`);
            err.code = 'ERR_FS_EISDIR';
            throw err;
        }
        mkdirSync(dest, { recursive: true });
        for (const name of readdirSync(src)) {
            cpSyncImpl(path.join(src, name), path.join(dest, name), options);
        }
        return;
    }
    if (options.errorOnExist && existsSync(dest)) {
        const err = new Error(`${dest} already exists`);
        err.code = 'ERR_FS_CP_EEXIST';
        throw err;
    }
    if (stats.isSymbolicLink()) {
        symlinkSync(readlinkSync(src), dest);
        return;
    }
    copyFileSync(src, dest);
}

export const cpSync = globalThis.fs
    ? (src, dest, options) => cpSyncImpl(
        String(normalizePath(src)), String(normalizePath(dest)), options || {})
    : unsupported('cpSync');

export const cp = globalThis.fs
    ? function cp(src, dest, optionsOrCallback, maybeCallback) {
        const callback = typeof optionsOrCallback === 'function'
            ? optionsOrCallback
            : maybeCallback;
        const options = typeof optionsOrCallback === 'function'
            ? {}
            : optionsOrCallback;
        if (typeof callback !== 'function') {
            throw new TypeError('fs.cp: callback must be a function');
        }
        try {
            cpSync(src, dest, options);
        } catch (error) {
            queueMicrotask(() => callback(error));
            return;
        }
        queueMicrotask(() => callback(null));
    }
    : unsupported('cp');

export const openSync = unsupported('openSync');
export const closeSync = unsupported('closeSync');
export const readSync = unsupported('readSync');
export const watch = unsupported('watch');
export const createReadStream = unsupported('createReadStream');
export const createWriteStream = unsupported('createWriteStream');

// Node-style callback API over the host promise surface.
export const readFile = hostFs('readFile')
    ? function readFile(path, optionsOrCallback, maybeCallback) {
        const callback = typeof optionsOrCallback === 'function'
            ? optionsOrCallback
            : maybeCallback;
        const options = typeof optionsOrCallback === 'function'
            ? undefined
            : optionsOrCallback;
        if (typeof callback !== 'function') {
            throw new TypeError('fs.readFile: callback must be a function');
        }
        hostFs('readFile')(normalizePath(path), options).then(
            (result) => callback(
                null, result instanceof Uint8Array ? Buffer.from(result) : result),
            (error) => callback(error),
        );
    }
    : unsupported('readFile');
export const writeFile = callbackify('writeFile', hostFs('writeFile'));
export const appendFile = callbackify('appendFile', hostFs('appendFile'));
export const mkdir = callbackify('mkdir', hostFs('mkdir'));
export const stat = callbackify('stat', hostFs('stat'));
export const lstat = callbackify('lstat', hostFs('lstat'));
export const readdir = callbackify('readdir', hostFs('readdir'));
export const rm = callbackify('rm', hostFs('rm'));
export const rmdir = callbackify('rmdir', hostFs('rmdir'));
export const unlink = callbackify('unlink', hostFs('unlink'));
export const rename = callbackify('rename', hostFs('rename'), 2);
export const copyFile = callbackify('copyFile', hostFs('copyFile'), 2);
export const readlink = callbackify('readlink', hostFs('readlink'));
export const symlink = callbackify('symlink', hostFs('symlink'), 2);

// The method set fs-consuming libraries bind at load time (isomorphic-git,
// globby, config loaders). node:fs/promises re-exports these names.
const PROMISE_METHODS = [
    'access', 'appendFile', 'chmod', 'chown', 'copyFile', 'cp', 'glob',
    'lchown', 'link', 'lstat', 'lutimes', 'mkdir', 'mkdtemp', 'open',
    'opendir', 'readdir', 'readFile', 'readlink', 'realpath', 'rename',
    'rm', 'rmdir', 'stat', 'statfs', 'symlink', 'truncate', 'unlink',
    'utimes', 'watch', 'writeFile',
];

export const constants = Object.freeze({
    F_OK: 0, R_OK: 4, W_OK: 2, X_OK: 1,
    O_RDONLY: 0, O_WRONLY: 1, O_RDWR: 2,
    O_CREAT: 64, O_EXCL: 128, O_TRUNC: 512, O_APPEND: 1024,
});

export const promises = { constants };
for (const name of PROMISE_METHODS) {
    const runtimeMethod = globalThis.fs && globalThis.fs[name];
    promises[name] = typeof runtimeMethod === 'function'
        ? (...args) => runtimeMethod.call(globalThis.fs, normalizePath(args[0]), ...args.slice(1))
        : () => Promise.reject(makeEnosys(name));
}
if (globalThis.fs) {
    promises.cp = (src, dest, options) => {
        try {
            cpSync(src, dest, options);
            return Promise.resolve();
        } catch (error) {
            return Promise.reject(error);
        }
    };
}

function normalizePath(value) {
    if (value instanceof URL && value.protocol === 'file:') value = decodeURIComponent(value.pathname);
    const path = String(value);
    const corpus = globalThis.__NODE_TEST_CORPUS_HOST__;
    return corpus && path.startsWith('/test/') ? corpus + path : value;
}

function makeEnosys(name) {
    const err = new Error(
        `fs.promises.${name} is not supported in this runtime; use the ` +
        'policy-gated fs capability (globalThis.fs) instead');
    err.code = 'ENOSYS';
    return err;
}

export default {
    readFile,
    readFileSync,
    writeFile,
    writeFileSync,
    appendFile,
    symlinkSync,
    mkdir,
    mkdirSync,
    stat,
    statSync,
    lstat,
    lstatSync,
    readdir,
    readdirSync,
    rm,
    rmSync,
    rmdir,
    rmdirSync,
    unlink,
    unlinkSync,
    rename,
    renameSync,
    copyFile,
    copyFileSync,
    cp,
    cpSync,
    readlink,
    readlinkSync,
    symlink,
    openSync,
    closeSync,
    readSync,
    watch,
    createReadStream,
    createWriteStream,
    existsSync,
    promises,
    constants,
};
