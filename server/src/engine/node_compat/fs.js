// node:fs — import-compatible stub. The sandbox's real filesystem surface
// is the policy-gated `fs` capability (global, see engine/fs.rs); this
// module exists so libraries whose unused code paths import node:fs (e.g.
// certificate-file loading in gRPC stacks) can load. Every operation
// throws.

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

export const readFile = unsupported('readFile');
export const readFileSync = unsupported('readFileSync');
export const writeFile = unsupported('writeFile');
export const writeFileSync = unsupported('writeFileSync');
export const openSync = unsupported('openSync');
export const closeSync = unsupported('closeSync');
export const readSync = unsupported('readSync');
export const watch = unsupported('watch');
export const createReadStream = unsupported('createReadStream');
export const createWriteStream = unsupported('createWriteStream');

export function existsSync() {
    return false;
}

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
    promises[name] = () => Promise.reject(makeEnosys(name));
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
