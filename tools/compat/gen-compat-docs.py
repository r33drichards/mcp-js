#!/usr/bin/env python3
"""Generate site-docs/reference/compatibility.md from the recorded compat
baselines: the WinterTC surface scan (surface_baseline.json), the WPT
expectations, and the Node core test expectations. Run after re-recording
any of those files; the page is committed so docs builds need no test run.
"""

import json
import pathlib
import collections

REPO = pathlib.Path(__file__).resolve().parents[2]
surface = json.loads((REPO / "server/tests/wpt/surface_baseline.json").read_text())
wpt = json.loads((REPO / "server/tests/wpt/expectations.json").read_text())
wpt_versions = json.loads((REPO / "server/tests/wpt/versions.json").read_text())
node = json.loads((REPO / "server/tests/node_compat/expectations.json").read_text())
node_versions = json.loads((REPO / "server/tests/node_compat/versions.json").read_text())

out = []
w = out.append

w("# Web & Node.js compatibility")
w("")
w("This page is generated from the compatibility test baselines in the")
w("repository (`tools/compat/gen-compat-docs.py`). Three suites gate every")
w("change in CI: a [WinterTC Minimum Common Web Platform API](https://min-common-api.proposal.wintertc.org/)")
w("surface scan, a vendored [web-platform-tests](https://github.com/web-platform-tests/wpt)")
w("subset, and a vendored subset of Node.js core tests running against the")
w("`node:` compatibility modules.")
w("")

# ── WinterTC surface ────────────────────────────────────────────────────
mca = surface["minCommonApi"]
w("## WinterTC Minimum Common API")
w("")
w(f"**{mca['present']} / {mca['total']} globals present.**")
w("")
w("| Global | Status |")
w("|---|---|")
for name, kind in sorted(mca["detail"].items()):
    status = "✅" if kind not in (None, "undefined") else "❌ missing"
    w(f"| `{name}` | {status} |")
w("")

# ── WPT ─────────────────────────────────────────────────────────────────
counts = collections.Counter()
suites = collections.defaultdict(lambda: [0, 0, 0])
subtest_failures = 0
for key, val in wpt.items():
    suite = key.split("/")[0]
    if val is True:
        counts["pass"] += 1
        suites[suite][0] += 1
    elif val is False:
        counts["fail"] += 1
        suites[suite][2] += 1
    elif val.get("ignore"):
        continue
    else:
        counts["partial"] += 1
        suites[suite][1] += 1
        subtest_failures += len(val.get("expectedFailures", []))

commit = wpt_versions["wpt"]["commit"][:9]
total = counts["pass"] + counts["partial"] + counts["fail"]
w(f"## Web Platform Tests (wpt@{commit})")
w("")
w(f"**{counts['pass']} / {total} vendored test files fully passing**")
w(f"({counts['partial']} with recorded subtest failures — {subtest_failures}")
w(f"individual subtests — and {counts['fail']} failing wholesale).")
w("")
w("| Suite | Fully passing | Partial | Failing |")
w("|---|---|---|---|")
for suite, (p, part, f) in sorted(suites.items()):
    w(f"| {suite} | {p} | {part} | {f} |")
w("")
w("Every entry is pinned in `server/tests/wpt/expectations.json`; CI fails")
w("on drift in either direction, so the file doubles as the compatibility")
w("changelog.")
w("")

# ── Node ────────────────────────────────────────────────────────────────
tag = node_versions["node"]["tag"]
npass = sum(1 for value in node.values() if value["status"] == "pass")
nrunnable = sum(1 for value in node.values() if value["status"] in ("pass", "fail"))
nclassified = {
    key: value
    for key, value in node.items()
    if value["status"] not in ("pass", "fail")
}
w(f"## Node.js core tests (node {tag})")
w("")
w(f"**{npass} / {nrunnable} vendored runnable tests passing.** The `node:` modules")
w("served by the module loader:")
w("")
w("| Module | Implementation |")
w("|---|---|")
w("| `node:assert` (+`/strict`) | purpose-written subset |")
w("| `node:buffer` | feross/buffer (the npm Buffer polyfill) |")
w("| `node:console` | the global console, plus a `Console` class over writable streams |")
w("| `node:crypto` | hash/HMAC/randomness subset over the sandbox crypto ops |")
w("| `node:dns` | pass-through resolver (resolution happens host-side in the transports) |")
w("| `node:events` | Node's own lib source over a primordials shim |")
w("| `node:fs` (+`/promises`) | import-compatible stubs; the real surface is the policy-gated `fs` global |")
w("| `node:http` | import-compatible stub; HTTP/1 is `fetch()` |")
w("| `node:http2` | client subset over the policy-gated http2 ops (gRPC transport) |")
w("| `node:https` | import-compatible stub; use `fetch()` or `node:http2` |")
w("| `node:module` | `createRequire`/`builtinModules` over the builtin registry |")
w("| `node:net` | address helpers; sockets are inert (transports are policy-gated) |")
w("| `node:os` | fixed sandbox values |")
w("| `node:path` | Node's own lib source over a primordials shim |")
w("| `node:process` | fixed sandbox values plus active timer/immediate resource snapshots; no host env |")
w("| `node:querystring` | Node's own lib source over a primordials shim |")
w("| `node:stream` | purpose-written subset (legacy `Stream` base + Readable/Writable/Duplex/Transform) |")
w("| `node:stream/web` | the runtime's WHATWG streams globals re-exported |")
w("| `node:timers` (+`/promises`) | the runtime timer globals, plus promisified forms with AbortSignal |")
w("| `node:tls` | option plumbing; TLS terminates host-side in the transports |")
w("| `node:url` | WHATWG URL + file-URL helpers |")
w("| `node:util` | purpose-written subset |")
w("| `node:zlib` | one-shot gzip/deflate over CompressionStream / DecompressionStream |")
w("")
if nclassified:
    w("Classified non-runnable tests (with reasons):")
    w("")
    for key, value in sorted(nclassified.items()):
        w(
            f"- `{key.split('/')[-1]}` — `{value['status']}` / "
            f"`{value['profile']}`: {value['reason']}"
        )
    w("")

w("## Known limitations")
w("")
w("- `SubtleCrypto` implements digest and raw-HMAC operations; asymmetric")
w("  keys, AES, and key derivation reject with `NotSupportedError`.")
w("- `node:crypto` covers hashes (md5/sha1/sha2 family), HMAC, randomness,")
w("  and `timingSafeEqual`; ciphers, sign/verify, key objects, and KDFs are")
w("  not exported.")
w("- Deep IDNA conformance (WPT IdnaTestV2) tracks the Rust `idna` crate")
w("  and is not vendored.")
w("- `fetch()` network behavior is governed by the engine's policy layer;")
w("  the WPT fetch suites covered here are the compute-only ones.")
w("")
w("Tracking issue: [#237](https://github.com/r33drichards/mcp-js/issues/237).")

(REPO / "site-docs/reference/compatibility.md").write_text("\n".join(out) + "\n")
print("wrote site-docs/reference/compatibility.md")
