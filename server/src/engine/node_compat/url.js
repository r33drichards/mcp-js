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

export default {
    URL, URLSearchParams, domainToASCII, domainToUnicode,
    fileURLToPath, pathToFileURL, urlToHttpOptions,
};
