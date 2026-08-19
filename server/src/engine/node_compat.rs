//! Node.js compatibility modules served through the module loader.
//!
//! `import ... from 'node:<name>'` resolves to an embedded ESM build:
//! path/querystring/events run Node v22.14.0's own lib sources against a
//! generated primordials shim (see tools/compat/gen-node-modules.py);
//! buffer bundles feross/buffer (the npm browser polyfill); the rest are
//! purpose-written subsets. `registry()` lists every served module so the
//! compat matrix in docs can be generated from code.

/// (module name, ESM source) for every supported `node:` module. Subpath
/// builtins (fs/promises, stream/web, timers/promises) are first-class
/// entries: the loader resolves the full path as the module name.
pub const NODE_MODULES: &[(&str, &str)] = &[
    ("assert", include_str!("node_compat/assert.js")),
    ("buffer", include_str!("node_compat/gen/buffer.js")),
    ("console", include_str!("node_compat/console.js")),
    ("crypto", include_str!("node_compat/crypto.js")),
    ("dns", include_str!("node_compat/dns.js")),
    ("dns/promises", include_str!("node_compat/dns_promises.js")),
    ("events", include_str!("node_compat/gen/events.js")),
    ("fs", include_str!("node_compat/fs.js")),
    ("fs/promises", include_str!("node_compat/fs_promises.js")),
    ("http", include_str!("node_compat/http.js")),
    ("http2", include_str!("node_compat/http2.js")),
    ("https", include_str!("node_compat/https.js")),
    ("module", include_str!("node_compat/module.js")),
    ("net", include_str!("node_compat/net.js")),
    ("os", include_str!("node_compat/os.js")),
    ("path", include_str!("node_compat/gen/path.js")),
    ("perf_hooks", include_str!("node_compat/perf_hooks.js")),
    ("process", include_str!("node_compat/process.js")),
    ("querystring", include_str!("node_compat/gen/querystring.js")),
    ("stream", include_str!("node_compat/stream.js")),
    ("stream/web", include_str!("node_compat/stream_web.js")),
    ("test", include_str!("node_compat/test.js")),
    ("timers", include_str!("node_compat/timers.js")),
    ("timers/promises", include_str!("node_compat/timers_promises.js")),
    ("tls", include_str!("node_compat/tls.js")),
    ("url", include_str!("node_compat/url.js")),
    ("util", include_str!("node_compat/util.js")),
    ("util/types", include_str!("node_compat/util_types.js")),
    ("zlib", include_str!("node_compat/zlib.js")),
];

pub fn source_for(name: &str) -> Option<&'static str> {
    NODE_MODULES
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, s)| *s)
}

/// Also accept `node:assert/strict` as an alias.
pub fn resolve_submodule(name: &str) -> Option<String> {
    if name == "assert/strict" {
        return Some(
            "import { strict } from 'node:assert';\nexport default strict;\n\
             export const { ok, equal, notEqual, strictEqual, notStrictEqual, \
             deepEqual, notDeepEqual, deepStrictEqual, notDeepStrictEqual, match, \
             doesNotMatch, throws, rejects, doesNotThrow, doesNotReject, fail, \
             ifError, AssertionError } = strict;\n"
                .to_string(),
        );
    }
    source_for(name).map(|s| s.to_string())
}
