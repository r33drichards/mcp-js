// URLPattern over the urlpattern crate (spec implementation; same
// integration shape as Deno's): Rust compiles the pattern to per-component
// ECMAScript regex source + canonicalizes match inputs, JS executes the
// regexes and builds the result objects.
(function () {
    'use strict';
    if (typeof globalThis.URLPattern === 'function' &&
        globalThis.URLPattern.__mcpV8Native === true) {
        return;
    }
    var opParse = Deno.core.ops.op_urlpattern_parse;
    var opProcessMatchInput = Deno.core.ops.op_urlpattern_process_match_input;

    var COMPONENTS = [
        'protocol', 'username', 'password', 'hostname',
        'port', 'pathname', 'search', 'hash',
    ];

    function normalizeInput(input, method) {
        if (typeof input === 'string') return input;
        if (input === null || (typeof input !== 'object' && typeof input !== 'function')) {
            throw new TypeError(
                "Failed to " + method + " 'URLPattern': parameter 1 is not of type 'URLPatternInit'.");
        }
        var out = {};
        for (var i = 0; i < COMPONENTS.length; i++) {
            var v = input[COMPONENTS[i]];
            if (v !== undefined) out[COMPONENTS[i]] = String(v);
        }
        if (input.baseURL !== undefined) out.baseURL = String(input.baseURL);
        return out;
    }

    var patternData = new WeakMap();

    function pdata(p) {
        var d = patternData.get(p);
        if (!d) throw new TypeError('Illegal invocation');
        return d;
    }

    class URLPattern {
        constructor(input, baseURLOrOptions, maybeOptions) {
            if (input === undefined) input = {};
            var baseURL, options;
            if (typeof baseURLOrOptions === 'string') {
                baseURL = baseURLOrOptions;
                options = maybeOptions === undefined ? {} : maybeOptions;
            } else {
                if (maybeOptions !== undefined) {
                    throw new TypeError(
                        "Failed to construct 'URLPattern': parameter 2 is not of type 'string'.");
                }
                baseURL = undefined;
                options = baseURLOrOptions === undefined ? {} : baseURLOrOptions;
            }
            var ignoreCase = !!options.ignoreCase;
            var compiled;
            try {
                compiled = opParse(normalizeInput(input, 'construct'), baseURL, ignoreCase);
            } catch (e) {
                throw new TypeError(
                    "Failed to construct 'URLPattern': " + (e && e.message ? e.message : e));
            }
            var regexps = {};
            // The URLPattern spec compiles component regexes with the 'v'
            // (unicodeSets) flag.
            var flags = ignoreCase ? 'vi' : 'v';
            for (var i = 0; i < COMPONENTS.length; i++) {
                var c = compiled[COMPONENTS[i]];
                try {
                    regexps[COMPONENTS[i]] = new RegExp(c.regexpString, flags);
                } catch (e) {
                    throw new TypeError(
                        "Failed to construct 'URLPattern': invalid regexp for " +
                        COMPONENTS[i] + ': ' + (e && e.message ? e.message : e));
                }
            }
            patternData.set(this, { compiled: compiled, regexps: regexps });
        }
        get protocol() { return pdata(this).compiled.protocol.patternString; }
        get username() { return pdata(this).compiled.username.patternString; }
        get password() { return pdata(this).compiled.password.patternString; }
        get hostname() { return pdata(this).compiled.hostname.patternString; }
        get port() { return pdata(this).compiled.port.patternString; }
        get pathname() { return pdata(this).compiled.pathname.patternString; }
        get search() { return pdata(this).compiled.search.patternString; }
        get hash() { return pdata(this).compiled.hash.patternString; }
        get hasRegExpGroups() { return pdata(this).compiled.hasRegexpGroups; }
        test(input, baseURL) {
            var d = pdata(this);
            if (input === undefined) input = {};
            var res;
            try {
                res = opProcessMatchInput(normalizeInput(input, "execute 'test' on"),
                    baseURL === undefined ? undefined : String(baseURL));
            } catch (e) {
                throw new TypeError(
                    "Failed to execute 'test' on 'URLPattern': " +
                    (e && e.message ? e.message : e));
            }
            if (!res) return false;
            var values = res[0];
            for (var i = 0; i < COMPONENTS.length; i++) {
                if (!d.regexps[COMPONENTS[i]].test(values[COMPONENTS[i]])) return false;
            }
            return true;
        }
        exec(input, baseURL) {
            var d = pdata(this);
            if (input === undefined) input = {};
            var res;
            try {
                res = opProcessMatchInput(normalizeInput(input, "execute 'exec' on"),
                    baseURL === undefined ? undefined : String(baseURL));
            } catch (e) {
                throw new TypeError(
                    "Failed to execute 'exec' on 'URLPattern': " +
                    (e && e.message ? e.message : e));
            }
            if (!res) return null;
            var values = res[0];
            var inputs = res[1];
            var result = {
                inputs: inputs[1] === undefined || inputs[1] === null
                    ? [inputs[0]] : [inputs[0], inputs[1]],
            };
            for (var i = 0; i < COMPONENTS.length; i++) {
                var name = COMPONENTS[i];
                var match = d.regexps[name].exec(values[name]);
                if (match === null) return null;
                var groups = {};
                var groupNames = d.compiled[name].groupNameList;
                for (var j = 0; j < groupNames.length; j++) {
                    groups[groupNames[j]] = match[j + 1];
                }
                result[name] = { input: values[name], groups: groups };
            }
            return result;
        }
    }
    Object.defineProperty(URLPattern.prototype, Symbol.toStringTag, {
        value: 'URLPattern', configurable: true,
    });
    Object.defineProperty(URLPattern, '__mcpV8Native', {
        value: true, configurable: true,
    });
    globalThis.URLPattern = URLPattern;
})();
