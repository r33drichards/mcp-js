# Vendored Web Platform Tests

This directory vendors a curated subset of the
[web-platform-tests](https://github.com/web-platform-tests/wpt) suite. The
implementation is **gated** on it: `tests/wpt_websocket.rs` runs every
`websockets/*.any.js` file inside the real Engine against a local echo server
and compares each file's outcome to `expectations.json`.

- A file that regresses (expected `pass`, now failing) fails CI.
- A file that starts passing (expected `fail`, now passing) also fails CI, so
  the expected-fail list cannot rot — update `expectations.json` in the same
  change that improves the implementation.

## Layout

- `resources/testharness.js` — the upstream WPT harness, unmodified.
- `websockets/constants.sub.js` — upstream helper; the runner substitutes the
  `{{host}}` / `{{ports[ws][0]}}` wptserve placeholders with the local echo
  server's address at run time.
- `websockets/*.any.js` — upstream test files, unmodified. Only the `?default`
  variant is exercised (no TLS, no h2 flags).
- `expectations.json` — `{ "<file>": "pass" | "fail" }`. Files not listed are
  expected to pass.

## Provenance

Fetched unmodified from
`https://raw.githubusercontent.com/web-platform-tests/wpt/master/` on
2026-08-17. WPT is licensed under the 3-Clause BSD License (see the upstream
repository's LICENSE.md).

## Refreshing / extending

To update or add files:

```bash
curl -O https://raw.githubusercontent.com/web-platform-tests/wpt/master/websockets/<file>.any.js
```

Keep the files byte-identical to upstream — behavioral divergence belongs in
`expectations.json`, not in edited test files. Tests that require wptserve
features beyond a plain `/echo` endpoint (subresource handlers, wss/TLS
certificates, HTTP/2) are out of scope for this runner; extend
`tests/wpt_websocket.rs` first if you want to vendor them.
