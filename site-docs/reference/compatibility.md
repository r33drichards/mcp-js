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

**222 / 224 vendored test files fully passing**
(2 with recorded subtest failures — 3
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
| url | 21 | 0 | 0 |
| urlpattern | 3 | 0 | 0 |
| wasm | 43 | 0 | 0 |
| xhr | 9 | 0 | 0 |

Every entry is pinned in `server/tests/wpt/expectations.json`; CI fails
on drift in either direction, so the file doubles as the compatibility
changelog.

## Node.js core tests (node v22.14.0)

**44 / 44 vendored runnable tests passing.** The `node:` modules
served by the module loader:

| Module | Implementation |
|---|---|
| `node:assert` (+`/strict`) | purpose-written subset |
| `node:buffer` | feross/buffer (the npm Buffer polyfill) |
| `node:console` | the global console, plus a `Console` class over writable streams |
| `node:crypto` | hash/HMAC/randomness subset over the sandbox crypto ops |
| `node:dns` | pass-through resolver (resolution happens host-side in the transports) |
| `node:events` | Node's own lib source over a primordials shim |
| `node:fs` (+`/promises`) | import-compatible stubs; the real surface is the policy-gated `fs` global |
| `node:http` | import-compatible stub; HTTP/1 is `fetch()` |
| `node:http2` | client subset over the policy-gated http2 ops (gRPC transport) |
| `node:https` | import-compatible stub; use `fetch()` or `node:http2` |
| `node:module` | `createRequire`/`builtinModules` over the builtin registry |
| `node:net` | address helpers; sockets are inert (transports are policy-gated) |
| `node:os` | fixed sandbox values |
| `node:path` | Node's own lib source over a primordials shim |
| `node:perf_hooks` | user timing, observers, and function timing over the shared performance timeline |
| `node:process` | fixed sandbox values plus active timer/immediate resource snapshots; no host env |
| `node:querystring` | Node's own lib source over a primordials shim |
| `node:stream` | purpose-written subset (legacy `Stream` base + Readable/Writable/Duplex/Transform) |
| `node:stream/web` | the runtime's WHATWG streams globals re-exported |
| `node:timers` (+`/promises`) | the runtime timer globals, plus promisified forms with AbortSignal |
| `node:tls` | option plumbing; TLS terminates host-side in the transports |
| `node:url` | WHATWG URL + file-URL helpers |
| `node:util` | purpose-written subset |
| `node:zlib` | CRC32 plus one-shot gzip/deflate over CompressionStream / DecompressionStream |

Classified non-runnable tests (with reasons):

- `test-events-once.js` — `harness_missing` / `pure`: requires node-internal module internal/event_target
- `test-path-resolve.js` — `policy_required` / `subprocess`: requires child_process to verify cwd-dependent resolution

## Known limitations

- `SubtleCrypto` implements digest and raw-HMAC operations; asymmetric
  keys, AES, and key derivation reject with `NotSupportedError`.
- `node:crypto` covers hashes (md5/sha1/sha2 family), HMAC, randomness,
  and `timingSafeEqual`; ciphers, sign/verify, key objects, and KDFs are
  not exported.
- Deep IDNA conformance (WPT IdnaTestV2) tracks the Rust `idna` crate
  and is not vendored.
- `fetch()` network behavior is governed by the engine's policy layer;
  the WPT fetch suites covered here are the compute-only ones.

Tracking issue: [#237](https://github.com/r33drichards/mcp-js/issues/237).
