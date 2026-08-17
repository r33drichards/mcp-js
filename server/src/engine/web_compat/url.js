// URL + URLSearchParams.
//
// URL parsing/serialization is delegated to the rust-url crate through two
// ops (op_url_parse / op_url_reparse) captured at injection time; the
// component strings come back spec-shaped. URLSearchParams is pure JS
// (application/x-www-form-urlencoded parse + serialize) and stays linked
// to its owning URL per the standard.
(function () {
    'use strict';
    if (typeof globalThis.URL === 'function' &&
        typeof globalThis.URLSearchParams === 'function') {
        return;
    }
    var opParse = Deno.core.ops.op_url_parse;
    var opReparse = Deno.core.ops.op_url_reparse;

    // USVString conversion: lone surrogates become U+FFFD.
    function toUSV(s) {
        return String(s).replace(
            /[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(^|[^\uD800-\uDBFF])[\uDC00-\uDFFF]/g,
            function (m) {
                if (m.length === 2) return m[0] + '\uFFFD';
                return '\uFFFD';
            });
    }

    var SETTERS = {
        protocol: 0, username: 1, password: 2, host: 3, hostname: 4,
        port: 5, pathname: 6, search: 7, hash: 8,
    };
    var FIELDS = [
        'href', 'protocol', 'username', 'password', 'host', 'hostname',
        'port', 'pathname', 'search', 'hash', 'origin',
    ];

    function isOpaquePath(parts) {
        // Opaque path: no authority and the path does not start with '/'.
        return parts.host === '' && parts.username === '' &&
            parts.pathname.length > 0 && parts.pathname[0] !== '/';
    }

    function rebuildOpaqueHref(parts) {
        parts.href = parts.protocol + parts.pathname + parts.search + parts.hash;
    }

    // Two spec rules rust-url does not implement yet:
    // 1. file: URLs normalize Windows drive letters written with '|'.
    // 2. In a URL with an opaque path followed by a query or fragment, a
    //    trailing space in the path serializes as %20.
    function fixupParts(parts) {
        if (parts.protocol === 'file:' && /^\/[A-Za-z]\|(?=\/|\?|#|$)/.test(parts.pathname)) {
            var fixed = '/' + parts.pathname[1] + ':' + parts.pathname.slice(3);
            parts.href = parts.href.replace(parts.pathname, fixed);
            parts.pathname = fixed;
        }
        if (isOpaquePath(parts) && (parts.search !== '' || parts.hash !== '') &&
            parts.pathname.length > 0 &&
            parts.pathname[parts.pathname.length - 1] === ' ') {
            parts.pathname = parts.pathname.slice(0, -1) + '%20';
            rebuildOpaqueHref(parts);
        }
        return parts;
    }

    function parseParts(input, base) {
        var joined = base === undefined ? opParse(input) : opParse(input, base);
        var values = joined.split('\n');
        var parts = {};
        for (var i = 0; i < FIELDS.length; i++) parts[FIELDS[i]] = values[i];
        return fixupParts(parts);
    }

    // ── application/x-www-form-urlencoded ───────────────────────────────
    function utf8PercentDecode(str) {
        var bytes = [];
        for (var i = 0; i < str.length; i++) {
            var c = str[i];
            if (c === '%' && i + 2 < str.length &&
                /^[0-9a-fA-F]{2}$/.test(str.substr(i + 1, 2))) {
                bytes.push(parseInt(str.substr(i + 1, 2), 16));
                i += 2;
            } else {
                var code = str.codePointAt(i);
                if (code > 0xffff) i++;
                // Encode the code point as UTF-8 bytes.
                if (code < 0x80) bytes.push(code);
                else if (code < 0x800) {
                    bytes.push(0xc0 | (code >> 6), 0x80 | (code & 0x3f));
                } else if (code < 0x10000) {
                    bytes.push(0xe0 | (code >> 12), 0x80 | ((code >> 6) & 0x3f),
                        0x80 | (code & 0x3f));
                } else {
                    bytes.push(0xf0 | (code >> 18), 0x80 | ((code >> 12) & 0x3f),
                        0x80 | ((code >> 6) & 0x3f), 0x80 | (code & 0x3f));
                }
            }
        }
        // "UTF-8 decode without BOM" per the spec.
        return new TextDecoder('utf-8', { ignoreBOM: true }).decode(new Uint8Array(bytes));
    }

    function parseFormUrlencoded(input) {
        var list = [];
        if (input === '') return list;
        var sequences = input.split('&');
        for (var i = 0; i < sequences.length; i++) {
            var seq = sequences[i];
            if (seq === '') continue;
            var eq = seq.indexOf('=');
            var name = eq === -1 ? seq : seq.slice(0, eq);
            var value = eq === -1 ? '' : seq.slice(eq + 1);
            name = utf8PercentDecode(name.replace(/\+/g, ' '));
            value = utf8PercentDecode(value.replace(/\+/g, ' '));
            list.push([name, value]);
        }
        return list;
    }

    function serializeComponent(str) {
        var out = '';
        var i = 0;
        while (i < str.length) {
            var code = str.codePointAt(i);
            i += code > 0xffff ? 2 : 1;
            var bytes;
            if (code < 0x80) bytes = [code];
            else if (code < 0x800) bytes = [0xc0 | (code >> 6), 0x80 | (code & 0x3f)];
            else if (code < 0x10000) {
                bytes = [0xe0 | (code >> 12), 0x80 | ((code >> 6) & 0x3f), 0x80 | (code & 0x3f)];
            } else {
                bytes = [0xf0 | (code >> 18), 0x80 | ((code >> 12) & 0x3f),
                    0x80 | ((code >> 6) & 0x3f), 0x80 | (code & 0x3f)];
            }
            for (var j = 0; j < bytes.length; j++) {
                var b = bytes[j];
                if (b === 0x20) out += '+';
                else if ((b >= 0x30 && b <= 0x39) || (b >= 0x41 && b <= 0x5a) ||
                         (b >= 0x61 && b <= 0x7a) ||
                         b === 0x2a || b === 0x2d || b === 0x2e || b === 0x5f) {
                    out += String.fromCharCode(b);
                } else {
                    out += '%' + (b < 16 ? '0' : '') + b.toString(16).toUpperCase();
                }
            }
        }
        return out;
    }

    function serializeParams(list) {
        var out = [];
        for (var i = 0; i < list.length; i++) {
            out.push(serializeComponent(list[i][0]) + '=' + serializeComponent(list[i][1]));
        }
        return out.join('&');
    }

    // ── URLSearchParams ─────────────────────────────────────────────────
    var uspData = new WeakMap();

    function usp(obj) {
        var d = uspData.get(obj);
        if (!d) throw new TypeError('Illegal invocation');
        return d;
    }

    // Called when the list mutates: write the query back into the owning URL.
    function uspUpdate(d) {
        if (!d.url) return;
        var query = serializeParams(d.list);
        applySetter(d.url, 'search', query, true);
    }

    class URLSearchParams {
        constructor(init) {
            var list = [];
            if (init !== undefined && init !== null) {
                if (typeof init === 'object' || typeof init === 'function') {
                    if (typeof init[Symbol.iterator] === 'function') {
                        var pairs = Array.from(init);
                        for (var i = 0; i < pairs.length; i++) {
                            var pair = Array.from(pairs[i]);
                            if (pair.length !== 2) {
                                throw new TypeError(
                                    "Failed to construct 'URLSearchParams': Sequence initializer must only contain pair elements.");
                            }
                            list.push([toUSV(pair[0]), toUSV(pair[1])]);
                        }
                    } else {
                        var keys = Object.keys(init);
                        for (var j = 0; j < keys.length; j++) {
                            var rk = toUSV(keys[j]);
                            var rv = toUSV(init[keys[j]]);
                            var found = false;
                            for (var m = 0; m < list.length; m++) {
                                if (list[m][0] === rk) {
                                    list[m][1] = rv;
                                    found = true;
                                    break;
                                }
                            }
                            if (!found) list.push([rk, rv]);
                        }
                    }
                } else {
                    var str = toUSV(init);
                    if (str.length > 0 && str[0] === '?') str = str.slice(1);
                    list = parseFormUrlencoded(str);
                }
            }
            uspData.set(this, { list: list, url: null });
        }
        append(name, value) {
            if (arguments.length < 2) {
                throw new TypeError(
                    "Failed to execute 'append': 2 arguments required, but only " +
                    arguments.length + ' present.');
            }
            var d = usp(this);
            d.list.push([toUSV(name), toUSV(value)]);
            uspUpdate(d);
        }
        delete(name, value) {
            if (arguments.length < 1) {
                throw new TypeError(
                    "Failed to execute 'delete': 1 argument required, but only 0 present.");
            }
            var d = usp(this);
            name = String(name);
            var hasValue = value !== undefined;
            if (hasValue) value = String(value);
            d.list = d.list.filter(function (p) {
                return !(p[0] === name && (!hasValue || p[1] === value));
            });
            uspUpdate(d);
        }
        get(name) {
            if (arguments.length < 1) {
                throw new TypeError(
                    "Failed to execute 'get': 1 argument required, but only 0 present.");
            }
            var d = usp(this);
            name = String(name);
            for (var i = 0; i < d.list.length; i++) {
                if (d.list[i][0] === name) return d.list[i][1];
            }
            return null;
        }
        getAll(name) {
            if (arguments.length < 1) {
                throw new TypeError(
                    "Failed to execute 'getAll': 1 argument required, but only 0 present.");
            }
            var d = usp(this);
            name = String(name);
            return d.list.filter(function (p) { return p[0] === name; })
                .map(function (p) { return p[1]; });
        }
        has(name, value) {
            if (arguments.length < 1) {
                throw new TypeError(
                    "Failed to execute 'has': 1 argument required, but only 0 present.");
            }
            var d = usp(this);
            name = String(name);
            var hasValue = value !== undefined;
            if (hasValue) value = String(value);
            for (var i = 0; i < d.list.length; i++) {
                if (d.list[i][0] === name && (!hasValue || d.list[i][1] === value)) {
                    return true;
                }
            }
            return false;
        }
        set(name, value) {
            if (arguments.length < 2) {
                throw new TypeError(
                    "Failed to execute 'set': 2 arguments required, but only " +
                    arguments.length + ' present.');
            }
            var d = usp(this);
            name = toUSV(name);
            value = toUSV(value);
            var found = false;
            var out = [];
            for (var i = 0; i < d.list.length; i++) {
                if (d.list[i][0] === name) {
                    if (!found) {
                        out.push([name, value]);
                        found = true;
                    }
                } else {
                    out.push(d.list[i]);
                }
            }
            if (!found) out.push([name, value]);
            d.list = out;
            uspUpdate(d);
        }
        sort() {
            var d = usp(this);
            // Stable sort by name, comparing UTF-16 code units.
            d.list = d.list
                .map(function (p, i) { return { p: p, i: i }; })
                .sort(function (a, b) {
                    if (a.p[0] < b.p[0]) return -1;
                    if (a.p[0] > b.p[0]) return 1;
                    return a.i - b.i;
                })
                .map(function (e) { return e.p; });
            uspUpdate(d);
        }
        get size() { return usp(this).list.length; }
        forEach(callback, thisArg) {
            if (arguments.length < 1) {
                throw new TypeError(
                    "Failed to execute 'forEach': 1 argument required, but only 0 present.");
            }
            if (typeof callback !== 'function') {
                throw new TypeError(
                    "Failed to execute 'forEach': parameter 1 is not of type 'Function'.");
            }
            var d = usp(this);
            for (var i = 0; i < d.list.length; i++) {
                callback.call(thisArg, d.list[i][1], d.list[i][0], this);
            }
        }
        toString() { return serializeParams(usp(this).list); }
    }

    function makeIterator(target, kind) {
        var index = 0;
        var iter = Object.create(uspIteratorPrototype);
        iter.__next = function () {
            var d = usp(target);
            if (index >= d.list.length) return { value: undefined, done: true };
            var pair = d.list[index++];
            var value = kind === 'key' ? pair[0] :
                kind === 'value' ? pair[1] : [pair[0], pair[1]];
            return { value: value, done: false };
        };
        return iter;
    }
    var uspIteratorPrototype = Object.create(
        Object.getPrototypeOf(Object.getPrototypeOf([][Symbol.iterator]())));
    uspIteratorPrototype.next = function next() { return this.__next(); };
    Object.defineProperty(uspIteratorPrototype, Symbol.toStringTag, {
        value: 'URLSearchParams Iterator', configurable: true,
    });

    URLSearchParams.prototype.entries = function entries() {
        return makeIterator(this, 'key+value');
    };
    URLSearchParams.prototype.keys = function keys() {
        return makeIterator(this, 'key');
    };
    URLSearchParams.prototype.values = function values() {
        return makeIterator(this, 'value');
    };
    URLSearchParams.prototype[Symbol.iterator] = URLSearchParams.prototype.entries;
    Object.defineProperty(URLSearchParams.prototype, Symbol.toStringTag, {
        value: 'URLSearchParams', configurable: true,
    });
    globalThis.URLSearchParams = URLSearchParams;

    // ── URL ─────────────────────────────────────────────────────────────
    var urlData = new WeakMap();

    function udata(obj) {
        var d = urlData.get(obj);
        if (!d) throw new TypeError('Illegal invocation');
        return d;
    }

    function applySetter(urlObj, field, value, fromParams) {
        var d = udata(urlObj);
        try {
            d.parts = (function () {
                var joined = opReparse(d.parts.href, SETTERS[field], String(value));
                var values = joined.split('\n');
                var parts = {};
                for (var i = 0; i < FIELDS.length; i++) parts[FIELDS[i]] = values[i];
                // Removing the query/fragment of an opaque path percent-
                // encodes a trailing space so the URL round-trips.
                if ((field === 'search' || field === 'hash') && String(value) === '' &&
                    parts.host === '' && parts.username === '' &&
                    parts.pathname.length > 0 && parts.pathname[0] !== '/' &&
                    parts.pathname[parts.pathname.length - 1] === ' ') {
                    parts.pathname = parts.pathname.slice(0, -1) + '%20';
                    parts.href = parts.protocol + parts.pathname + parts.search + parts.hash;
                }
                return fixupParts(parts);
            })();
        } catch (_) {
            return; // failed setters are silent no-ops
        }
        if (!fromParams && d.searchParams) {
            var pd = uspData.get(d.searchParams);
            pd.list = parseFormUrlencoded(
                d.parts.search.length > 0 ? d.parts.search.slice(1) : '');
        }
    }

    class URL {
        constructor(url, base) {
            if (arguments.length < 1) {
                throw new TypeError(
                    "Failed to construct 'URL': 1 argument required, but only 0 present.");
            }
            // Argument conversion happens before parsing: a throwing
            // toString propagates as-is, not as "Invalid URL".
            var urlStr = String(url);
            var baseStr = base === undefined ? undefined : String(base);
            var parts;
            try {
                parts = parseParts(urlStr, baseStr);
            } catch (e) {
                throw new TypeError(e && e.message ? e.message : 'Invalid URL');
            }
            urlData.set(this, { parts: parts, searchParams: null });
        }
        static parse(url, base) {
            if (arguments.length < 1) {
                throw new TypeError(
                    "Failed to execute 'parse': 1 argument required, but only 0 present.");
            }
            try {
                return new URL(url, base);
            } catch (_) {
                return null;
            }
        }
        static canParse(url, base) {
            if (arguments.length < 1) {
                throw new TypeError(
                    "Failed to execute 'canParse': 1 argument required, but only 0 present.");
            }
            try {
                parseParts(String(url), base === undefined ? undefined : String(base));
                return true;
            } catch (_) {
                return false;
            }
        }
        get href() { return udata(this).parts.href; }
        set href(v) {
            var d = udata(this);
            var parts;
            try {
                parts = parseParts(String(v));
            } catch (e) {
                throw new TypeError(e && e.message ? e.message : 'Invalid URL');
            }
            d.parts = parts;
            if (d.searchParams) {
                var pd = uspData.get(d.searchParams);
                pd.list = parseFormUrlencoded(
                    parts.search.length > 0 ? parts.search.slice(1) : '');
            }
        }
        get origin() { return udata(this).parts.origin; }
        get protocol() { return udata(this).parts.protocol; }
        set protocol(v) { applySetter(this, 'protocol', v); }
        get username() { return udata(this).parts.username; }
        set username(v) { applySetter(this, 'username', v); }
        get password() { return udata(this).parts.password; }
        set password(v) { applySetter(this, 'password', v); }
        get host() { return udata(this).parts.host; }
        set host(v) { applySetter(this, 'host', v); }
        get hostname() { return udata(this).parts.hostname; }
        set hostname(v) { applySetter(this, 'hostname', v); }
        get port() { return udata(this).parts.port; }
        set port(v) { applySetter(this, 'port', v); }
        get pathname() { return udata(this).parts.pathname; }
        set pathname(v) { applySetter(this, 'pathname', v); }
        get search() { return udata(this).parts.search; }
        set search(v) { applySetter(this, 'search', v); }
        get searchParams() {
            var d = udata(this);
            if (!d.searchParams) {
                // Parse the raw query directly: the public constructor's
                // leading-'?' strip must not apply to a query that itself
                // starts with '?'.
                var params = new URLSearchParams();
                uspData.get(params).list = parseFormUrlencoded(
                    d.parts.search.length > 0 ? d.parts.search.slice(1) : '');
                uspData.get(params).url = this;
                d.searchParams = params;
            }
            return d.searchParams;
        }
        get hash() { return udata(this).parts.hash; }
        set hash(v) { applySetter(this, 'hash', v); }
        toString() { return udata(this).parts.href; }
        toJSON() { return udata(this).parts.href; }
    }
    Object.defineProperty(URL.prototype, Symbol.toStringTag, {
        value: 'URL', configurable: true,
    });
    globalThis.URL = URL;
})();
