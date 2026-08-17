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

export const promises = {
    readFile: (...args) => Promise.reject(makeEnosys('readFile')),
    writeFile: (...args) => Promise.reject(makeEnosys('writeFile')),
    stat: (...args) => Promise.reject(makeEnosys('stat')),
};

function makeEnosys(name) {
    const err = new Error(`fs.promises.${name} is not supported in this runtime`);
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
};
