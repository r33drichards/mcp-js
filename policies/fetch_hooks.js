// Example JavaScript pre/post hooks for fetch() — the same behavior as
// fetch_hooks.rego, written in JS. Wire up with:
//
//   --policies-json '{
//     "fetch": {
//       "policies": [{"url": "file:///etc/policies/fetch.rego"}],
//       "pre":      [{"url": "file:///etc/policies/fetch_hooks.js"}],
//       "post":     [{"url": "file:///etc/policies/fetch_hooks.js"}]
//     }
//   }'
//
// A JS hook file runs in its own bare V8 isolate with no host capabilities
// (no fetch, no fs — pure computation), kept warm across calls so top-level
// state persists. Functions must be synchronous. Return semantics:
//   - undefined / null            → abstain (allow, no change)
//   - true / false                → allow / deny
//   - {allow, reason, input}      → pre: deny with reason, or rewrite input
//   - {allow, reason, output}     → post: deny with reason, or rewrite output

function pre(input) {
    // Refuse requests that smuggle credentials in the query string.
    if (input.url_parsed.query.toLowerCase().includes("api_key=")) {
        return { allow: false, reason: "credentials in query string" };
    }
    // Upgrade http:// to https:// before the policy sees it.
    if (input.url.startsWith("http://")) {
        return { input: { ...input, url: "https://" + input.url.slice(7) } };
    }
}

function post(input, output) {
    // Strip a sensitive header from responses before JS sees them.
    if (output.headers && "x-internal-trace" in output.headers) {
        const { "x-internal-trace": _dropped, ...headers } = output.headers;
        return { output: { ...output, headers } };
    }
}
