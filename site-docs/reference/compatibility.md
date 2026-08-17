# Web & Node.js compatibility

This page is generated from the compatibility test baselines in the
repository (`tools/compat/gen-compat-docs.py`). Three suites gate every
change in CI: a [WinterTC Minimum Common Web Platform API](https://min-common-api.proposal.wintertc.org/)
surface scan, a vendored [web-platform-tests](https://github.com/web-platform-tests/wpt)
subset, and a vendored subset of Node.js core tests running against the
`node:` compatibility modules.

## WinterTC Minimum Common API

**54 / 54 globals present.**

| Global | Status |
|---|---|
| `AbortController` | ✅ |
| `AbortSignal` | ✅ |
| `Blob` | ✅ |
| `ByteLengthQueuingStrategy` | ✅ |
| `CompressionStream` | ✅ |
| `CountQueuingStrategy` | ✅ |
| `Crypto` | ✅ |
| `CryptoKey` | ✅ |
| `CustomEvent` | ✅ |
| `DOMException` | ✅ |
| `DecompressionStream` | ✅ |
| `ErrorEvent` | ✅ |
| `Event` | ✅ |
| `EventTarget` | ✅ |
| `File` | ✅ |
| `FormData` | ✅ |
| `Headers` | ✅ |
| `MessageChannel` | ✅ |
| `MessageEvent` | ✅ |
| `MessagePort` | ✅ |
| `Performance` | ✅ |
| `PromiseRejectionEvent` | ✅ |
| `ReadableStream` | ✅ |
| `ReadableStreamBYOBReader` | ✅ |
| `ReadableStreamDefaultReader` | ✅ |
| `Request` | ✅ |
| `Response` | ✅ |
| `SubtleCrypto` | ✅ |
| `TextDecoder` | ✅ |
| `TextDecoderStream` | ✅ |
| `TextEncoder` | ✅ |
| `TextEncoderStream` | ✅ |
| `TransformStream` | ✅ |
| `URL` | ✅ |
| `URLPattern` | ✅ |
| `URLSearchParams` | ✅ |
| `WebAssembly` | ✅ |
| `WritableStream` | ✅ |
| `WritableStreamDefaultWriter` | ✅ |
| `atob` | ✅ |
| `btoa` | ✅ |
| `clearInterval` | ✅ |
| `clearTimeout` | ✅ |
| `console` | ✅ |
| `crypto` | ✅ |
| `fetch` | ✅ |
| `navigator` | ✅ |
| `performance` | ✅ |
| `queueMicrotask` | ✅ |
| `reportError` | ✅ |
| `self` | ✅ |
| `setInterval` | ✅ |
| `setTimeout` | ✅ |
| `structuredClone` | ✅ |

## Web Platform Tests (wpt@0cc6a7e19)

**218 / 224 vendored test files fully passing**
(6 with recorded subtest failures — 94
individual subtests — and 0 failing wholesale).

| Suite | Fully passing | Partial | Failing |
|---|---|---|---|
| FileAPI | 14 | 0 | 0 |
| WebCryptoAPI | 3 | 0 | 0 |
| compression | 18 | 0 | 0 |
| console | 6 | 0 | 0 |
| dom | 13 | 0 | 0 |
| encoding | 6 | 0 | 0 |
| fetch | 8 | 0 | 0 |
| hr-time | 2 | 0 | 0 |
| html | 11 | 1 | 0 |
| streams | 65 | 1 | 0 |
| url | 18 | 3 | 0 |
| urlpattern | 3 | 0 | 0 |
| wasm | 42 | 1 | 0 |
| xhr | 9 | 0 | 0 |

Every entry is pinned in `server/tests/wpt/expectations.json`; CI fails
on drift in either direction, so the file doubles as the compatibility
changelog.

## Node.js core tests (node v22.14.0)

**25 / 25 vendored tests passing.** The `node:` modules
served by the module loader:

| Module | Implementation |
|---|---|
| `node:path` | Node's own lib source over a primordials shim |
| `node:querystring` | Node's own lib source over a primordials shim |
| `node:events` | Node's own lib source over a primordials shim |
| `node:buffer` | feross/buffer (the npm Buffer polyfill) |
| `node:assert` (+`/strict`) | purpose-written subset |
| `node:util` | purpose-written subset |
| `node:url` | WHATWG URL + file-URL helpers |
| `node:process` | fixed sandbox values; no host env |
| `node:os` | fixed sandbox values |

Skipped tests (with reasons):

- `test-event-emitter-prepend.js` — requires node:stream (not implemented)
- `test-events-once.js` — pokes node-internal module internal/event_target
- `test-path-resolve.js` — requires child_process for cwd checks

## Known limitations

- `SubtleCrypto` implements digest and raw-HMAC operations; asymmetric
  keys, AES, and key derivation reject with `NotSupportedError`.
- Deep IDNA conformance (WPT IdnaTestV2) tracks the Rust `idna` crate
  and is not vendored.
- `fetch()` network behavior is governed by the engine's policy layer;
  the WPT fetch suites covered here are the compute-only ones.

Tracking issue: [#237](https://github.com/r33drichards/mcp-js/issues/237).
