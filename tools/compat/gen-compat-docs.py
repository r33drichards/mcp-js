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
npass = sum(1 for v in node.values() if v is True)
nignore = {k: v for k, v in node.items() if isinstance(v, dict) and v.get("ignore")}
ntotal = len(node) - len(nignore)
w(f"## Node.js core tests (node {tag})")
w("")
w(f"**{npass} / {ntotal} vendored tests passing.** The `node:` modules")
w("served by the module loader:")
w("")
w("| Module | Implementation |")
w("|---|---|")
w("| `node:path` | Node's own lib source over a primordials shim |")
w("| `node:querystring` | Node's own lib source over a primordials shim |")
w("| `node:events` | Node's own lib source over a primordials shim |")
w("| `node:buffer` | feross/buffer (the npm Buffer polyfill) |")
w("| `node:assert` (+`/strict`) | purpose-written subset |")
w("| `node:crypto` | hash/HMAC/randomness subset over the sandbox crypto ops |")
w("| `node:util` | purpose-written subset |")
w("| `node:url` | WHATWG URL + file-URL helpers |")
w("| `node:process` | fixed sandbox values; no host env |")
w("| `node:os` | fixed sandbox values |")
w("")
if nignore:
    w("Skipped tests (with reasons):")
    w("")
    for k, v in sorted(nignore.items()):
        w(f"- `{k.split('/')[-1]}` — {v.get('reason', 'ignored')}")
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
