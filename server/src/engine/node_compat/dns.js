// node:dns — pass-through resolver. Real name resolution happens host-side
// when a policy-gated transport (fetch / WebSocket / node:http2) dials the
// authority, so lookup() answers with the hostname itself as the "address".
// Callers that dial whatever lookup returned (the @grpc/grpc-js pattern)
// end up handing the hostname straight back to the transport, which keeps
// TLS SNI and host-scoped policy/header-injection rules intact — and no
// separate DNS side channel is exposed to the isolate.

import { isIP } from 'node:net';

function lookupResult(hostname) {
    const family = isIP(hostname) === 6 ? 6 : 4;
    return { address: hostname, family };
}

export function lookup(hostname, options, callback) {
    if (typeof options === 'function') {
        callback = options;
        options = undefined;
    }
    const all = !!(options && typeof options === 'object' && options.all);
    const result = lookupResult(hostname);
    Promise.resolve().then(() => {
        if (all) callback(null, [result]);
        else callback(null, result.address, result.family);
    });
}

function notFound(hostname, syscall) {
    const err = new Error(`${syscall} ENOTFOUND ${hostname}`);
    err.code = 'ENOTFOUND';
    err.syscall = syscall;
    err.hostname = hostname;
    return err;
}

export function resolveTxt(_hostname, callback) {
    // No TXT records: gRPC service-config lookups fall back to defaults.
    Promise.resolve().then(() => callback(null, []));
}

export function resolveSrv(hostname, callback) {
    Promise.resolve().then(() => callback(notFound(hostname, 'resolveSrv')));
}

class Resolver {
    setServers(_servers) {}
    getServers() { return []; }
    resolve4(hostname) { return promises.resolve4(hostname); }
    resolve6(hostname) { return promises.resolve6(hostname); }
    resolveTxt(hostname) { return promises.resolveTxt(hostname); }
    resolveSrv(hostname) { return promises.resolveSrv(hostname); }
    cancel() {}
}

export const promises = {
    Resolver,
    lookup(hostname, options) {
        const all = !!(options && typeof options === 'object' && options.all);
        const result = lookupResult(hostname);
        return Promise.resolve(all ? [result] : result);
    },
    resolveTxt(_hostname) {
        return Promise.resolve([]);
    },
    resolveSrv(hostname) {
        return Promise.reject(notFound(hostname, 'resolveSrv'));
    },
    resolve4(hostname) {
        return isIP(hostname) === 4
            ? Promise.resolve([hostname])
            : Promise.reject(notFound(hostname, 'resolve4'));
    },
    resolve6(hostname) {
        return isIP(hostname) === 6
            ? Promise.resolve([hostname])
            : Promise.reject(notFound(hostname, 'resolve6'));
    },
};

export default { lookup, resolveTxt, resolveSrv, promises };
