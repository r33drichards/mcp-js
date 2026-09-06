// node:url — the WHATWG surface (URL/URLSearchParams re-exported from the
// web globals) plus the path <-> file-URL helpers. The legacy
// url.parse()/format() API is intentionally absent.
import path from 'node:path';

export const URL = globalThis.URL;
export const URLSearchParams = globalThis.URLSearchParams;

export function domainToASCII(domain) {
    try {
        return new URL('http://' + domain).hostname;
    } catch {
        return '';
    }
}

export function domainToUnicode(domain) {
    // Without a punycode decoder we return the ASCII form.
    return domainToASCII(domain);
}

export function fileURLToPath(url) {
    const u = typeof url === 'string' ? new URL(url) : url;
    if (!u || u.protocol !== 'file:') {
        const err = new TypeError('The URL must be of scheme file');
        err.code = 'ERR_INVALID_URL_SCHEME';
        throw err;
    }
    if (u.hostname !== '' && u.hostname !== 'localhost') {
        const err = new TypeError('File URL host must be "localhost" or empty');
        err.code = 'ERR_INVALID_FILE_URL_HOST';
        throw err;
    }
    let pathname = u.pathname;
    if (/%2f/i.test(pathname)) {
        const err = new TypeError('File URL path must not include encoded / characters');
        err.code = 'ERR_INVALID_FILE_URL_PATH';
        throw err;
    }
    return decodeURIComponent(pathname);
}

export function pathToFileURL(filepath) {
    const resolved = path.resolve(String(filepath));
    const url = new URL('file://');
    let encoded = '';
    for (const c of resolved) {
        if (c === '%') encoded += '%25';
        else if (c === '\n') encoded += '%0A';
        else if (c === '\r') encoded += '%0D';
        else if (c === '\t') encoded += '%09';
        else if (c === '#') encoded += '%23';
        else if (c === '?') encoded += '%3F';
        else encoded += c;
    }
    url.pathname = encoded;
    if (String(filepath).endsWith('/') && !url.pathname.endsWith('/')) {
        url.pathname += '/';
    }
    return url;
}

export function urlToHttpOptions(url) {
    return {
        protocol: url.protocol,
        hostname: url.hostname.startsWith('[') ? url.hostname.slice(1, -1) : url.hostname,
        hash: url.hash,
        search: url.search,
        pathname: url.pathname,
        path: url.pathname + url.search,
        href: url.href,
        port: url.port !== '' ? Number(url.port) : undefined,
        auth: url.username || url.password
            ? `${decodeURIComponent(url.username)}:${decodeURIComponent(url.password)}` : undefined,
    };
}

// ── legacy url API (url.parse / format / resolve) ───────────────────────
// A WHATWG-backed approximation of the legacy parser: covers absolute URLs
// and the host-less path form (`/foo?bar`) the http corpus feeds it.

function legacyObject(u, slashes) {
    const query = u.search ? u.search.slice(1) : null;
    const auth = u.username || u.password
        ? `${decodeURIComponent(u.username)}${u.password ? ':' + decodeURIComponent(u.password) : ''}`
        : null;
    return {
        protocol: u.protocol || null,
        slashes,
        auth,
        host: u.host || null,
        port: u.port !== '' ? u.port : null,
        hostname: u.hostname || null,
        hash: u.hash || null,
        search: u.search || null,
        query,
        pathname: u.pathname || null,
        path: (u.pathname || '') + (u.search || '') || null,
        href: u.href,
    };
}

export function parse(input, _parseQueryString, _slashesDenoteHost) {
    if (typeof input !== 'string') {
        const err = new TypeError(
            `The "url" argument must be of type string. Received type ${typeof input}`);
        err.code = 'ERR_INVALID_ARG_TYPE';
        throw err;
    }
    try {
        const u = new globalThis.URL(input);
        return legacyObject(u, input.includes('//'));
    } catch {
        // Host-less form: parse relative to a throwaway base and blank out
        // the host fields.
        try {
            const u = new globalThis.URL(input, 'placeholder://placeholder-host');
            const query = u.search ? u.search.slice(1) : null;
            return {
                protocol: null,
                slashes: null,
                auth: null,
                host: null,
                port: null,
                hostname: null,
                hash: u.hash || null,
                search: u.search || null,
                query,
                pathname: u.pathname || null,
                path: (u.pathname || '') + (u.search || '') || null,
                href: (u.pathname || '') + (u.search || '') + (u.hash || ''),
            };
        } catch {
            const err = new TypeError(`Invalid URL: ${input}`);
            err.code = 'ERR_INVALID_URL';
            throw err;
        }
    }
}

export function format(input) {
    if (typeof input === 'string') return input;
    if (input instanceof globalThis.URL) return input.href;
    if (input && typeof input === 'object') {
        if (typeof input.href === 'string') return input.href;
        const protocol = input.protocol
            ? (input.protocol.endsWith(':') ? input.protocol : input.protocol + ':')
            : '';
        const host = input.host
            || (input.hostname
                ? input.hostname + (input.port ? ':' + input.port : '')
                : '');
        const auth = input.auth ? input.auth + '@' : '';
        const pathname = input.pathname || '';
        const search = input.search
            || (input.query
                ? '?' + (typeof input.query === 'string'
                    ? input.query
                    : new globalThis.URLSearchParams(input.query).toString())
                : '');
        const hash = input.hash || '';
        const slashes = input.slashes || host ? '//' : '';
        return `${protocol}${slashes}${auth}${host}${pathname}${search}${hash}`;
    }
    return String(input);
}

export function resolve(from, to) {
    try {
        return new globalThis.URL(to, from).href;
    } catch {
        return to;
    }
}

export const Url = function Url() {};

export default {
    URL, URLSearchParams, domainToASCII, domainToUnicode,
    fileURLToPath, pathToFileURL, urlToHttpOptions,
    parse, format, resolve, Url,
};
