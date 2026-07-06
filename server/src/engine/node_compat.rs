//! CommonJS `require()` compatibility shim for Node built-ins.
//!
//! mcp-v8 runs code as an ES module in a bare V8 isolate, so Node's module
//! system does not exist. Agents nevertheless write Node-flavored code by
//! habit (`require('fs')`, `require('path')`), and without this shim the
//! failure is an opaque `ReferenceError: require is not defined`.
//!
//! `inject_require` installs a `require()` global that resolves a small,
//! fixed set of built-ins:
//!
//! * `fs` / `node:fs` — the sandbox `fs` global (only when the server was
//!   started with a filesystem policy; otherwise `require('fs')` throws an
//!   error explaining how to enable it)
//! * `fs/promises` / `node:fs/promises` — `fs.promises`
//! * `path` / `node:path` / `path/posix` — a pure-JS POSIX implementation
//!   of Node's `path` module (the sandbox has no notion of Windows paths;
//!   `path.posix` is a self-reference)
//!
//! Anything else throws `MODULE_NOT_FOUND` with a message that says what IS
//! supported and points npm users at ES-module imports. The shim grants no
//! capability of its own: `fs` stays policy-gated exactly as before, and the
//! check happens lazily at call time so `require` itself can always exist.

use deno_core::JsRuntime;

/// Install the `require()` global. Always injected (like `atob`/`console`):
/// resolution of capability-backed modules is checked at call time, so this
/// is safe to run regardless of which policies are configured. Must run
/// before `console::harden_runtime`.
pub fn inject_require(runtime: &mut JsRuntime) -> Result<(), String> {
    runtime
        .execute_script("<require-setup>", REQUIRE_JS_WRAPPER.to_string())
        .map_err(|e| format!("Failed to install require shim: {}", e))?;
    Ok(())
}

const REQUIRE_JS_WRAPPER: &str = r#"
(function() {
    // ── path: a POSIX-only port of Node's `path` module ─────────────────

    function assertPath(p, fn) {
        if (typeof p !== 'string') throw new TypeError('path.' + fn + ': path must be a string');
    }

    // Collapse '.' and '' segments and apply '..'. When `allowAboveRoot` is
    // true (relative input), leading '..' segments are preserved; when false
    // (absolute input), they cannot climb past the root and are dropped.
    function normalizeParts(parts, allowAboveRoot) {
        const res = [];
        for (const p of parts) {
            if (!p || p === '.') continue;
            if (p === '..') {
                if (res.length && res[res.length - 1] !== '..') res.pop();
                else if (allowAboveRoot) res.push('..');
            } else res.push(p);
        }
        return res;
    }

    function isAbsolute(p) {
        assertPath(p, 'isAbsolute');
        return p.length > 0 && p.charCodeAt(0) === 47;
    }

    function normalize(p) {
        assertPath(p, 'normalize');
        if (p.length === 0) return '.';
        const abs = p.charCodeAt(0) === 47;
        const trailing = p.length > 1 && p.charCodeAt(p.length - 1) === 47;
        let out = normalizeParts(p.split('/'), !abs).join('/');
        if (!out && !abs) out = '.';
        if (out && trailing) out += '/';
        return abs ? '/' + out : out;
    }

    function join() {
        if (arguments.length === 0) return '.';
        let joined;
        for (const a of arguments) {
            assertPath(a, 'join');
            if (a.length > 0) joined = joined === undefined ? a : joined + '/' + a;
        }
        if (joined === undefined) return '.';
        return normalize(joined);
    }

    // The sandbox has no working directory; resolution roots at '/'.
    function resolve() {
        let resolved = '';
        let abs = false;
        for (let i = arguments.length - 1; i >= 0 && !abs; i--) {
            const p = arguments[i];
            assertPath(p, 'resolve');
            if (p.length === 0) continue;
            resolved = resolved ? p + '/' + resolved : p;
            abs = p.charCodeAt(0) === 47;
        }
        return '/' + normalizeParts(resolved.split('/'), false).join('/');
    }

    function dirname(p) {
        assertPath(p, 'dirname');
        if (p.length === 0) return '.';
        const abs = p.charCodeAt(0) === 47;
        let end = -1, matchedSlash = true;
        for (let i = p.length - 1; i >= 1; --i) {
            if (p.charCodeAt(i) === 47) {
                if (!matchedSlash) { end = i; break; }
            } else matchedSlash = false;
        }
        if (end === -1) return abs ? '/' : '.';
        if (abs && end === 1) return '//';
        return p.slice(0, end);
    }

    // A direct port of Node's posix basename, including its suffix-stripping
    // loop (basename('.txt', '.txt') is '' on Node, not '.txt').
    function basename(p, suffix) {
        assertPath(p, 'basename');
        if (suffix !== undefined) assertPath(suffix, 'basename');
        let start = 0, end = -1, matchedSlash = true;
        if (suffix !== undefined && suffix.length > 0 && suffix.length <= p.length) {
            if (suffix === p) return '';
            let extIdx = suffix.length - 1;
            let firstNonSlashEnd = -1;
            for (let i = p.length - 1; i >= 0; --i) {
                if (p.charCodeAt(i) === 47) {
                    if (!matchedSlash) { start = i + 1; break; }
                } else {
                    if (firstNonSlashEnd === -1) { matchedSlash = false; firstNonSlashEnd = i + 1; }
                    if (extIdx >= 0) {
                        if (p.charCodeAt(i) === suffix.charCodeAt(extIdx)) {
                            if (--extIdx === -1) end = i;
                        } else { extIdx = -1; end = firstNonSlashEnd; }
                    }
                }
            }
            if (start === end) end = firstNonSlashEnd;
            else if (end === -1) end = p.length;
            return p.slice(start, end);
        }
        for (let i = p.length - 1; i >= 0; --i) {
            if (p.charCodeAt(i) === 47) {
                if (!matchedSlash) { start = i + 1; break; }
            } else if (end === -1) { matchedSlash = false; end = i + 1; }
        }
        if (end === -1) return '';
        return p.slice(start, end);
    }

    function extname(p) {
        assertPath(p, 'extname');
        const base = basename(p);
        if (base === '..') return '';
        const dot = base.lastIndexOf('.');
        // dot <= 0 covers "no dot" and a leading dot ('.bashrc' has no extension)
        if (dot <= 0) return '';
        return base.slice(dot);
    }

    function relative(from, to) {
        assertPath(from, 'relative');
        assertPath(to, 'relative');
        if (from === to) return '';
        from = resolve(from);
        to = resolve(to);
        if (from === to) return '';
        const fromParts = from.split('/').filter(Boolean);
        const toParts = to.split('/').filter(Boolean);
        let i = 0;
        while (i < fromParts.length && i < toParts.length && fromParts[i] === toParts[i]) i++;
        const up = fromParts.slice(i).map(function() { return '..'; });
        return up.concat(toParts.slice(i)).join('/');
    }

    // A direct port of Node's posix parse — including its quirks (e.g.
    // parse('/..') yields name '.', ext '.'), so code written against Node
    // sees identical values here.
    function parse(p) {
        assertPath(p, 'parse');
        const ret = { root: '', dir: '', base: '', ext: '', name: '' };
        if (p.length === 0) return ret;
        const abs = p.charCodeAt(0) === 47;
        let start;
        if (abs) { ret.root = '/'; start = 1; } else { start = 0; }
        let startDot = -1, startPart = 0, end = -1, matchedSlash = true, preDotState = 0;
        for (let i = p.length - 1; i >= start; --i) {
            const code = p.charCodeAt(i);
            if (code === 47) {
                if (!matchedSlash) { startPart = i + 1; break; }
                continue;
            }
            if (end === -1) { matchedSlash = false; end = i + 1; }
            if (code === 46) {
                if (startDot === -1) startDot = i;
                else if (preDotState !== 1) preDotState = 1;
            } else if (startDot !== -1) {
                preDotState = -1;
            }
        }
        if (end !== -1) {
            const s = startPart === 0 && abs ? 1 : startPart;
            if (startDot === -1 || preDotState === 0 ||
                (preDotState === 1 && startDot === end - 1 && startDot === startPart + 1)) {
                ret.base = ret.name = p.slice(s, end);
            } else {
                ret.name = p.slice(s, startDot);
                ret.base = p.slice(s, end);
                ret.ext = p.slice(startDot, end);
            }
        }
        if (startPart > 0) ret.dir = p.slice(0, startPart - 1);
        else if (abs) ret.dir = '/';
        return ret;
    }

    function format(o) {
        if (o === null || typeof o !== 'object') {
            throw new TypeError('path.format: argument must be an object');
        }
        const dir = o.dir || o.root || '';
        const base = o.base || ((o.name || '') + (o.ext || ''));
        if (!dir) return base;
        return dir === o.root ? dir + base : dir + '/' + base;
    }

    const path = {
        sep: '/', delimiter: ':',
        isAbsolute: isAbsolute, normalize: normalize, join: join, resolve: resolve,
        dirname: dirname, basename: basename, extname: extname, relative: relative,
        parse: parse, format: format,
        toNamespacedPath: function(p) { return p; },
    };
    path.posix = path;

    // ── require() ────────────────────────────────────────────────────────

    const BUILTINS = "fs, fs/promises, path";

    // The bare built-in name for a supported specifier ('node:fs' -> 'fs'),
    // or null when the specifier is not a supported built-in.
    function builtinName(spec) {
        if (typeof spec !== 'string' || spec.length === 0) return null;
        const name = spec.startsWith('node:') ? spec.slice(5) : spec;
        if (name === 'fs' || name === 'fs/promises' || name === 'path' || name === 'path/posix') return name;
        return null;
    }

    function notFound(spec) {
        const name = String(spec);
        let hint;
        if (name.startsWith('./') || name.startsWith('../') || name.startsWith('/')) {
            hint = "CommonJS file modules are not supported; code runs as a single ES module.";
        } else {
            hint = "For npm packages, use an ES module import (e.g. `import _ from 'npm:lodash-es'`) — available when the server allows external imports.";
        }
        const err = new Error(
            "Cannot find module '" + name + "'. mcp-v8 is not Node.js: require() resolves only these built-ins: "
            + BUILTINS + ". " + hint
        );
        err.code = 'MODULE_NOT_FOUND';
        throw err;
    }

    function requireShim(spec) {
        if (typeof spec !== 'string' || spec.length === 0) {
            throw new TypeError('require: module specifier must be a non-empty string');
        }
        const name = builtinName(spec);
        if (name === null) notFound(spec);
        if (name === 'path' || name === 'path/posix') return path;
        // fs is capability-gated: the global exists only when the server was
        // started with a filesystem policy. Checked lazily, at call time.
        if (typeof globalThis.fs === 'undefined') {
            throw new Error(
                "Cannot find module '" + spec + "': filesystem access is disabled on this server. "
                + "Start mcp-v8 with a filesystem policy (--policies-json '{\"filesystem\":{...}}') to enable the fs module."
            );
        }
        return name === 'fs' ? globalThis.fs : globalThis.fs.promises;
    }

    // Node returns the specifier itself for built-ins. Resolvability does not
    // depend on the fs capability: 'fs' is a known module even when disabled.
    requireShim.resolve = function(spec) {
        if (builtinName(spec) === null) notFound(spec);
        return spec;
    };

    globalThis.require = requireShim;
})();
"#;
