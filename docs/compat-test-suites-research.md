# Research: test suites for exploring Node.js and browser compatibility

*Status: research notes / proposal — no code changes yet.*
*Date: 2026-08-17*

This document surveys the conformance and compatibility test suites that could
be integrated into mcp-js to measure how close the `run_js` runtime is to
Node.js and to browsers, how peer runtimes (Node, Deno, Bun, workerd, undici)
run those suites, and proposes a phased integration plan.

## 1. Where mcp-v8 stands today

The engine (deno_core 0.381, `server/src/engine/`) deliberately exposes a small
global surface, all of it hand-rolled JS bootstrap rather than the stock
`deno_web`/`deno_url` extension crates:

| Present | Notes |
|---|---|
| `console` | custom (`console.rs`) |
| `setTimeout` / `clearTimeout` | no `setInterval` (`timers.rs`) |
| `fetch` | custom op + JS bootstrap (`fetch.rs`) |
| `Headers` | hand-rolled prototype inside the fetch bootstrap; not the spec class (no iterator protocol, no name validation) |
| `Blob`, `File`, `FormData` | injected always |
| `TextEncoder` / `TextDecoder` | hand-rolled, UTF-8 only — no encoding-label table, no streaming |
| `atob` / `btoa` | injected always |
| `WebAssembly` | V8-native |
| `fs` | Node-*flavored* (`fs.promises`, `Stats` predicates, `err.code` — contract locked in by `server/tests/fs_node_compat.rs` for isomorphic-git) |
| `child_process`, `mcp` | custom host APIs |
| `npm:` / `jsr:` / URL imports | via esm.sh in the module loader |

**Absent:** `URL`/`URLSearchParams`/`URLPattern`, `Request`/`Response` as
globals, all WHATWG streams, `crypto`/`SubtleCrypto`, `AbortController`,
`Event`/`EventTarget`, `DOMException`, `structuredClone`, `queueMicrotask`,
`setInterval`, `performance`, `CompressionStream`/`DecompressionStream`,
`navigator`, `self`, and every `node:` builtin.

This shapes everything below: the interesting compat gap is in **runtime/web
APIs**, not the language — the language is stock V8.

## 2. The suite landscape

### 2.1 Web Platform Tests (WPT) — the main event for browser compat

- Repo: <https://github.com/web-platform-tests/wpt>; docs: <https://web-platform-tests.org/>.
- The only test type relevant to a headless runtime is **testharness.js
  tests** (`.any.js`, `.window.js`, `.worker.js`). Reftests, crashtests,
  wdspec, and `testdriver.js`-dependent tests need a browser and are skipped by
  every server-side runtime.
- `.any.js` tests default to running in window + dedicated-worker scopes; the
  harness expects `self`, a `GLOBAL` object (`isWindow()`/`isWorker()`
  sniffing), timers, and Promise microtasks. WPT even has a `jsshell` scope
  (used by SpiderMonkey) proving non-browser embedding is supported upstream.
- Directory → API map for a minimal runtime, and whether the WPT Python server
  (`wptserve`) is required:

  | WPT dir | API | Needs wptserve? |
  |---|---|---|
  | `html/webappapis/atob`, `timers`, `microtask-queuing` | atob/btoa, timers, queueMicrotask | no |
  | `console/` | console | no |
  | `encoding/` | TextEncoder/Decoder + full label table | mostly no |
  | `url/`, `urlpattern/` | URL family (data-driven from `urltestdata.json`) | no (needs a resource-loader shim) |
  | `FileAPI/blob`, `FileAPI/file`, `xhr/formdata` | Blob/File/FormData | mostly no |
  | `streams/`, `compression/` | WHATWG streams, Compression | mostly no |
  | `WebCryptoAPI/` | crypto.subtle | no |
  | `wasm/jsapi/` | WebAssembly JS API | no |
  | `fetch/api/headers`, `fetch/data-urls` | Headers, data: URLs | no |
  | `fetch/api/` (basic/redirect/cors), `websockets/`, `wasm/webapi/` | network behavior | **yes** (multi-origin, `.py` handlers, template substitution) |

**How peers run WPT:**

- **Node core** (`test/wpt/` + `test/common/wpt.js` + vendored subsets in
  `test/fixtures/wpt/` with `versions.json` pinning): no HTTP server at all —
  a `ResourceLoader` maps `/resources/...` URLs to disk, `location` is a mock,
  `// META:` is parsed with ~30 lines of regex, and per-suite status files
  (`test/wpt/status/*.json`) record expected failures. Daily wpt.fyi upload via
  `.github/workflows/daily-wpt-fyi.yml`.
- **Deno** (`tests/wpt/`, submodule of their WPT fork): spawns the real Python
  `wpt serve`, fetches each generated wrapper page, flattens it to a JS bundle,
  swaps in its own `testharnessreport.js`, and runs it in a `deno run`
  subprocess. Expectations in JSON mirroring the tree
  (`true` / `false` / `{expectedFailures}` / `{ignore}`), with an `update`
  command that regenerates them from a run. Uploads to wpt.fyi as product
  `deno`. **Notably, Deno runs WPT *instead of* Test262.**
- **undici** (Node's fetch): WPT as a git submodule + real wptserve + an
  `expectation.json`, process-per-test.
- **workerd** (`src/wpt/`): reimplements the testharness API in TypeScript, no
  server (template placeholders string-interpolated, resources served from
  Bazel-bound files), per-suite config where every deviation requires a
  `comment`, and generators for config/report/stats.
- **Bun**: no WPT runner — hand-written tests, some ported from WPT data.
- **wpt.fyi** accepts server runtimes as first-class products: `deno` and
  `node.js` runs are live today (verify:
  `https://wpt.fyi/api/runs?product=node.js`). Reports use the
  `wptreport.json` format.

**Honest estimate for mcp-v8 today:** roughly **500–600 test files (a few
thousand subtests) are runnable without wptserve** given the current globals.
Expected early results: `wasm/jsapi` near Chrome-level (V8-native — the
single cheapest win), `atob`/`console` mostly green, `encoding/` under ~20%
(UTF-8-only decoder), `FileAPI`/`formdata` mixed, `fetch/api/headers` mostly
red (non-spec `Headers`). That spread is exactly the value: it turns "compat"
into a scoreboard.

### 2.2 WinterTC Minimum Common API — the target list

- WinterCG became **Ecma TC55 ("WinterTC")** in January 2025; the **Minimum
  Common Web Platform API** is now a real Ecma standard (first edition = "2025
  snapshot", adopted December 2025; living draft:
  <https://min-common-api.proposal.wintertc.org/>).
- The API list is effectively a roadmap checklist for mcp-v8: fetch +
  Request/Response/Headers/FormData, Blob/File, full WHATWG streams,
  Compression streams, TextEncoder/Decoder (+stream variants), atob/btoa, URL/
  URLSearchParams/URLPattern, crypto/SubtleCrypto, AbortController/AbortSignal,
  Event/EventTarget, DOMException, MessageChannel, structuredClone,
  queueMicrotask, performance, timers incl. `setInterval`, `reportError`,
  `self`, `navigator.userAgent`, and the WebAssembly namespace incl. streaming
  compile. mcp-v8 currently covers roughly a third.
- There is **no WinterTC conformance suite yet** (no test repo in the
  WinterTC55 org; "wintercg/wpt-runtime" does not exist). An active 2025–26
  workstream is curating a WPT subset matching the Minimum Common API, with
  fixes upstreamed to WPT (TPAC 2025 session, Igalia). Practical consequence:
  vendor WPT directories yourself now, swap to the WinterTC list when it
  lands.
- Related: **WinterTC runtime-keys** (<https://runtime-keys.proposal.wintercg.org/>)
  defines the runtime identifier namespace (`node`, `deno`, `bun`, `workerd`,
  …) used by ecosystem tooling.

### 2.3 Node.js compatibility suites

Three methodologies, in rising order of cost:

1. **API-surface introspection** — **cloudflare/workers-nodejs-compat-matrix**
   (<https://github.com/cloudflare/workers-nodejs-compat-matrix>, report at
   workers-nodejs-compat-matrix.pages.dev). Per-runtime dump scripts walk every
   `node:*` module's export tree and record `supported`/`mock`/`stub`/`missing`
   per symbol vs a Node baseline, rendered as a diff matrix. Near-zero
   prerequisites — a single `run_js` script can produce the same JSON shape.
2. **Vendored Node core test subset** — the Deno and Bun approach:
   - **Deno**: `tests/node_compat/` with a `config.jsonc` pass-list
     (~2,000 entries: path → `{ignore, reason, flaky, platform flags, env}`),
     Node's `test/` dir vendored as the `denoland/node_test` submodule
     (tracking Node 26.x), public dashboard at
     <https://node-test-viewer.deno.dev/> (Deno 2.8: 76.4%, 3,405/4,457 of a
     selected subset).
   - **Bun**: vendors + *edits* Node tests at `test/js/node/test/`
     (`parallel/`, `common/`, …), runs them on every commit since Bun 1.2,
     ports Node's `test/common` harness, stubs `internalBinding` tests, and
     relaxes exact-error-message asserts to `name` + `code`. Per-module pass
     rates published in their docs.
   - **Node's own suite**: `test/parallel` is the bulk; every test needs
     `require('../common')`, `node:assert`, `process`, `Buffer`, exit-code
     semantics; a minority need `--expose-internals` and are unusable without
     rewrites. MIT-licensed; vendoring with attribution headers is established
     practice (both Deno and Bun do it publicly).
   - Least-coupled families to start with, should mcp-v8 ever want this:
     `test-path-*`, `test-querystring-*`, `test-url-*`, `test-buffer-*`,
     `test-assert-*` — they need only `require`, `assert`, `Buffer`, and
     minimal `process`.
3. **npm-package smoke tests** — **nodejs/citgm** (Node's release-gating
   "run popular packages' own test suites") and **oven-sh/bun-ecosystem-ci**.
   These presuppose npm, `child_process.spawn(process.execPath)`, and most of
   Node — the wrong tier for an embedder with bespoke APIs.

Supporting layers: **unjs/unenv** (runtime-agnostic `node:` polyfills/mocks —
what Cloudflare mixes with native code for `nodejs_compat_v2`) is the cheapest
way to move matrix cells from "missing" to "mock/supported" through the
existing module loader; **workerd** tests its compat layer with bespoke tests
(`src/workerd/api/node/tests/`), not Node's suite wholesale — a validation
that partial, curated compat is a legitimate posture.

### 2.4 Runtime compat matrices (feature-detection datasets)

- **unjs/runtime-compat** (<https://github.com/unjs/runtime-compat>, site
  <https://runtime-compat.unjs.io/>, npm `runtime-compat-data`): MDN
  browser-compat-data-format support tables for **web APIs** across
  bun/deno/node/workerd/llrt/fastly/netlify/edge-light/wasmer. Data is
  generated by actually executing feature-detection probes (derived from
  **openwebdocs/mdn-bcd-collector**, modified to run outside browsers) in each
  runtime via per-runtime runners under `generator/runtimes/<runtime>/`.
  Adding mcp-v8 = one runner directory invoking `run_js` — instant
  comparability against every major server runtime with zero test authoring.
- **MDN BCD** itself tracks `nodejs` and `deno` as first-class "browsers";
  Bun/workerd live only in runtime-compat. Known caveats: stub false
  positives, and URL/Request probes that needed `location.href` fixes for
  non-browser runtimes (mdn-bcd-collector PR #1295).
- No MCP server or embedded-V8 product publishes a compat matrix today —
  doing so would be a genuine differentiator for mcp-js.

### 2.5 Test262 — worth a subset, for a different reason

- <https://github.com/tc39/test262>: ~53,600 test files (plus strict/sloppy
  doubling ≈ 90k executions), BSD-3-Clause, ~280 MB tree. Runner contract in
  `INTERPRETING.md`: a `$262` host object (`createRealm`, `detachArrayBuffer`,
  `evalScript`, `gc`, `agent`, …), `assert.js`/`sta.js` includes, async tests
  via the `print` + `Test262:AsyncTestComplete` protocol, negative tests with
  phase (parse/resolution/runtime) + error type, flags
  (`module`/`raw`/`onlyStrict`/…).
- Best structural templates for a Rust in-process runner: **Boa**'s
  `tests/tester` (auto-clone, frontmatter parsing, rayon parallelism, TOML
  ignore config, `compare` subcommand; dashboard at boajs.dev/conformance) and
  **Nova**'s `tests/` (submodule pin + `expectations.json` + `skip.json`; CI
  fails on *any* drift in either direction). A dormant `test262-harness` crate
  exists for frontmatter parsing. Cross-engine dashboard: <https://test262.fyi/>.
- **Honest signal assessment:** the score belongs to V8, not mcp-js — Deno
  skips Test262 entirely for exactly this reason. What a subset *does* buy an
  embedder is narrow but real: a **tripwire for snapshot/heap-restore
  corruption** (a mangled heap restore lights up thousands of built-ins tests
  at once — uniquely relevant to mcp-v8's content-addressed heap snapshots),
  plus detection of V8 flag/build drift and bootstrap global pollution across
  `deno_core`/`rusty_v8` bumps. It says nothing about Node/browser API
  closeness.

### 2.6 Suites to skip (and why)

- **WebAssembly spec tests** (`WebAssembly/spec` `test/core`, wast2json
  workflow): core wasm semantics are V8's job and V8's CI already runs them.
  The embedder-owned surface is the JS API → run **WPT `wasm/jsapi/`**
  instead (and `wasm/webapi/` once streaming compile + Response exist).
- **kangax compat-table / node.green**: measure ECMAScript features per
  engine version — stock V8 maxes them; at most a one-off sanity check of
  deno_core's V8 flags.
- **web-platform-tests/interop, caniuse**: browser-engines only.
- **CITGM / ecosystem CI**: wrong tier until (if ever) mcp-v8 has npm install
  + process spawning + a broad `node:` surface.

## 3. Proposed phased integration

**Phase 0 — surface scan (days).** A `run_js` script that recursively dumps
`globalThis` (and attempts each `node:*` import) and diffs against the
Node/browser/workerd baselines from cloudflare/workers-nodejs-compat-matrix
plus the WinterTC Minimum Common API list. Output: a checked-in JSON +
generated markdown "what exists / what's missing" page. Near-zero cost,
immediately honest, auto-regenerable in CI.

**Phase 1 — vendored WPT subset, Node-style (the core investment).**
- Vendor (not submodule) `resources/` (testharness.js) plus the serverless
  directories: `html/webappapis/{atob,timers,microtask-queuing}`, `console/`,
  `encoding/`, `url/`, `FileAPI/{blob,file}`, `xhr/formdata`, `wasm/jsapi/`,
  `fetch/api/headers`, `fetch/data-urls` — with a `versions.json` pin like
  Node's `test/fixtures/wpt`.
- Runner (Rust test bin or orchestrator over the engine): parse `// META:`
  lines; bootstrap `self`/`window` aliases, a `GLOBAL` sniffing object, a mock
  `location`, and a Node-style `ResourceLoader` mapping `/resources/...` to
  disk; load real testharness.js + a custom `testharnessreport.js` that ships
  per-subtest JSON out of the isolate (Deno's `#$#$#`-prefixed-line trick works
  over the console channel); pump the event loop for async tests.
- Deno-format expectation files (`true`/`false`/`{expectedFailures}`/
  `{ignore}`) with an `--update` mode; CI fails on drift in either direction.
- Later: spawn the Python `wpt serve` (undici/Deno-style) to unlock
  `fetch/api/{basic,redirect,cors}` and `websockets/`; emit `wptreport.json`
  so results are wpt.fyi-uploadable (server runtimes are accepted products).

**Phase 2 — publish comparability.** Add an mcp-v8 runner to
unjs/runtime-compat's generator (or run it privately first) to appear in the
BCD-format matrix next to Node/Deno/Bun/workerd; optionally upload
`wptreport.json` to wpt.fyi. No other MCP runtime publishes this — it's a
differentiator.

**Phase 3 — Node compat, deliberately scoped.** If/when a `node:` facade is
desired, source it from **unenv** backed by the existing host bindings (the
`fs` object is already Node-flavored), then vendor a ~100–300-test Node core
subset Deno-style (`config.jsonc` with `ignore`/`reason`, attribution headers)
starting from the least-coupled families (`path`, `querystring`, `url`,
`buffer`, `assert`). Prereqs: CJS `require` (or a transform), Node-flavored
`assert`, `Buffer`, a `common` shim, minimal `process` with exit-code
semantics.

**Optional — Test262 subset as a snapshot tripwire.** Nightly (not per-PR)
run of `test/built-ins` + `test/language` (skip `intl402`/`staging`/`annexB`,
`IsHTMLDDA`, `agent`, `createRealm`) against a `JsRuntime` restored from a
snapshot, with a Nova-style expectations file. Value: catches heap-restore /
bootstrap / V8-flag regressions broadly; explicitly not a conformance
scoreboard.

## 4. Key sources

- WPT: repo & docs (<https://web-platform-tests.org/>); Node runner
  `test/common/wpt.js` + `test/fixtures/wpt` + `test/wpt/status/`; Deno
  `tests/wpt/` (runner.ts, expectations, wpt.fyi uploads); undici
  `test/web-platform-tests/`; workerd `src/wpt/`; domenic/wpt-runner.
- WinterTC: <https://min-common-api.proposal.wintertc.org/>,
  WinterTC55/proposal-minimum-common-api, runtime-keys, TPAC 2025 session
  (Igalia) on the WPT-subset workstream.
- Node compat: bun.sh/blog/bun-v1.2; oven-sh/bun `test/js/node/test/`;
  denoland/deno `tests/node_compat` + denoland/node_test;
  node-test-viewer.deno.dev; nodejs/node `test/README.md` and
  `test/common/README.md`; cloudflare/workers-nodejs-compat-matrix;
  unjs/unenv; nodejs/citgm.
- Matrices: unjs/runtime-compat (+ npm `runtime-compat-data`);
  openwebdocs/mdn-bcd-collector (PR #1295); mdn/browser-compat-data.
- Test262: tc39/test262 (`INTERPRETING.md`, LICENSE); boa-dev/boa
  `tests/tester`; trynova/nova `tests/`; test262.fyi; V8 `test/test262/`
  (`test262.status`, harness-adapt shims).
