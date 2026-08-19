# URL implementation benchmark: whatwg-url (JS) vs a rust-url fork

The last large block of WPT failures (83 subtests across `url/`) is
parser-level: rust-url and idna, at their latest releases, track a slightly
older URL spec. Two candidate closes were prototyped and measured against
each other (issue #237, PR #238):

- **Option A — vendor whatwg-url**: bundle jsdom's `whatwg-url` 17.1.0
  (plus `tr46` 6.0.0, `webidl-conversions` 8.0.1, `punycode` 2.3.1) as a
  single injected file and use it as the `URL` / `URLSearchParams`
  implementation.
- **Option B — fork rust-url**: vendor the `url` crate into
  `server/vendor/url` behind `[patch.crates-io]` and patch the behaviors
  the spec changed.

Both prototypes are in-tree:

- `server/tests/wpt/runner/whatwg-url-bundle.js` — the bundle (built by
  the mini-bundler; `WPT_URL_IMPL=whatwg` runs the WPT suite against it).
- `server/vendor/url` — the fork, live in the build, carrying the
  whatwg/url#963 host-retention patch as a first installment.
- `server/tests/url_impl_bench.rs` — the A/B benchmark
  (`cargo test --test url_impl_bench -- --ignored --nocapture`).

## Correctness (WPT url/ suite, 21 files)

| | rust-url 2.5.8 (unpatched) | whatwg-url bundle | rust-url fork (one patch) |
|---|---|---|---|
| url/ subtest failures | 83 | **0 — all files green** | 72 |

whatwg-url is the reference implementation of the spec; a full green is
expected. The fork's first ~40-line patch (Windows drive letters no longer
erase a file URL's host, whatwg/url#963) closed 11 subtests; closing the
rest is estimated at 300–500 patch lines across `parser.rs` (file-URL
slash handling, `///` empty-host cases, `/.` path serialization,
non-special backslashes) plus an idna fork for the invalid-punycode
leniency cases (~14 subtests).

## Performance (url_impl_bench, same corpus, identical output checksums)

| metric | rust-url ops | whatwg-url bundle |
|---|---|---|
| bundle eval per fresh isolate | 0 ms | **~32 ms** |
| 300k parse+read roundtrips | ~9.4 s (≈32k/s) | ~17.4 s (≈17k/s) |
| 50k URLSearchParams cycles | ~3.4 s | ~4.1–5.8 s |
| injected source | — | +357 KiB |

The parse gap (~1.85x) is smaller than a pure JS-vs-Rust comparison would
suggest because every `new URL` already crosses the op boundary and
serializes all components. The decisive number is the **32 ms bundle eval
per isolate**: stateless executions build a fresh isolate per request, and
the load-test baseline for a whole stateless request is ~52 ms — option A
adds ~60% to that unless the bootstrap moves into a V8 snapshot.

## Decision matrix

| | whatwg-url | rust-url fork |
|---|---|---|
| url/ green | all of it, today | incremental, patch by patch |
| runtime cost | 32 ms/isolate + heap (or snapshot work) | none |
| maintenance | tracks spec upstream (jsdom) | in-repo parser patches to maintain |
| precedent | — | repo already forks deno_core via `[patch.crates-io]` |

## Status

The fork (option B) is live with its first patch; the whatwg-url bundle
stays as a test-only reference implementation that also pins down expected
behavior (`WPT_URL_IMPL=whatwg`). If the remaining fork patches turn out
heavier than estimated, the fallback is snapshotting the bootstrap and
shipping the bundle instead.
