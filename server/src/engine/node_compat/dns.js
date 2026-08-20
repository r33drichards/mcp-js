// node:dns — pass-through resolver. Real name resolution happens host-side
// when a policy-gated transport (fetch / WebSocket / node:http2) dials the
// authority, so lookup() answers with the hostname itself as the "address".
// Callers that dial whatever lookup returned (the @grpc/grpc-js pattern)
// end up handing the hostname straight back to the transport, which keeps
// TLS SNI and host-scoped policy/header-injection rules intact — and no
// separate DNS side channel is exposed to the isolate.

import { isIP } from 'node:net';

export const NODATA = 'ENODATA';
export const FORMERR = 'EFORMERR';
export const SERVFAIL = 'ESERVFAIL';
export const NOTFOUND = 'ENOTFOUND';
export const NOTIMP = 'ENOTIMP';
export const REFUSED = 'EREFUSED';
export const BADQUERY = 'EBADQUERY';
export const BADNAME = 'EBADNAME';
export const BADFAMILY = 'EBADFAMILY';
export const BADRESP = 'EBADRESP';
export const CONNREFUSED = 'ECONNREFUSED';
export const TIMEOUT = 'ETIMEOUT';
export const EOF = 'EOF';
export const FILE = 'EFILE';
export const NOMEM = 'ENOMEM';
export const DESTRUCTION = 'EDESTRUCTION';
export const BADSTR = 'EBADSTR';
export const BADFLAGS = 'EBADFLAGS';
export const NONAME = 'ENONAME';
export const BADHINTS = 'EBADHINTS';
export const NOTINITIALIZED = 'ENOTINITIALIZED';
export const LOADIPHLPAPI = 'ELOADIPHLPAPI';
export const ADDRGETNETWORKPARAMS = 'EADDRGETNETWORKPARAMS';
export const CANCELLED = 'ECANCELLED';
export const ADDRCONFIG = 32;
export const V4MAPPED = 8;
export const ALL = 16;

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

function invalidAddress(address) {
    const err = new TypeError(
        `The argument 'address' is invalid. Received '${address}'`);
    err.code = 'ERR_INVALID_ARG_VALUE';
    return err;
}

function missingLookupServiceArgs(callbackRequired) {
    const names = callbackRequired
        ? '"address", "port", and "callback"'
        : '"address" and "port"';
    const err = new TypeError(`The ${names} arguments must be specified`);
    err.code = 'ERR_MISSING_ARGS';
    return err;
}

function invalidPort(port) {
    const err = new RangeError(
        'Port should be >= 0 and < 65536. Received ' + String(port) + '.');
    err.code = 'ERR_SOCKET_BAD_PORT';
    return err;
}

function validateLookupService(address, port, callback, callbackRequired) {
    if (address === undefined || port === undefined ||
        (callbackRequired && callback === undefined)) {
        throw missingLookupServiceArgs(callbackRequired);
    }
    if (typeof address !== 'string' || isIP(address) === 0) {
        throw invalidAddress(address);
    }
    const numericPort = typeof port === 'string' && port.trim() !== ''
        ? Number(port)
        : port;
    if (typeof numericPort !== 'number' || !Number.isInteger(numericPort) ||
        numericPort < 0 || numericPort >= 65536) {
        throw invalidPort(port);
    }
    if (callbackRequired && typeof callback !== 'function') {
        const err = new TypeError(
            'The "callback" argument must be of type function. Received ' +
            String(callback));
        err.code = 'ERR_INVALID_ARG_TYPE';
        throw err;
    }
    return numericPort;
}

function reverseLookup(address, port) {
    if (address === '::1' || address.startsWith('127.')) {
        return { hostname: 'localhost', service: String(port) };
    }
    if (address === '0.0.0.0' || address === '::') {
        return { hostname: address, service: String(port) };
    }
    throw notFound(address, 'getnameinfo');
}

function notFound(hostname, syscall) {
    const err = new Error(`${syscall} ENOTFOUND ${hostname}`);
    err.code = 'ENOTFOUND';
    err.syscall = syscall;
    err.hostname = hostname;
    return err;
}

export function lookupService(address, port, callback) {
    const numericPort = validateLookupService(address, port, callback, true);
    Promise.resolve().then(() => {
        try {
            const result = reverseLookup(address, numericPort);
            callback(null, result.hostname, result.service);
        } catch (error) {
            callback(error);
        }
    });
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
    lookupService(address, port) {
        const numericPort = validateLookupService(
            address, port, undefined, false);
        return Promise.resolve().then(() => reverseLookup(address, numericPort));
    },
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
    NODATA, FORMERR, SERVFAIL, NOTFOUND, NOTIMP, REFUSED,
    BADQUERY, BADNAME, BADFAMILY, BADRESP, CONNREFUSED, TIMEOUT,
    EOF, FILE, NOMEM, DESTRUCTION, BADSTR, BADFLAGS, NONAME,
    BADHINTS, NOTINITIALIZED, LOADIPHLPAPI, ADDRGETNETWORKPARAMS,
    CANCELLED, ADDRCONFIG, V4MAPPED, ALL,
};

export default {
    lookup, lookupService, resolveTxt, resolveSrv, promises,
    NODATA, FORMERR, SERVFAIL, NOTFOUND, NOTIMP, REFUSED,
    BADQUERY, BADNAME, BADFAMILY, BADRESP, CONNREFUSED, TIMEOUT,
    EOF, FILE, NOMEM, DESTRUCTION, BADSTR, BADFLAGS, NONAME,
    BADHINTS, NOTINITIALIZED, LOADIPHLPAPI, ADDRGETNETWORKPARAMS,
    CANCELLED, ADDRCONFIG, V4MAPPED, ALL,
};
