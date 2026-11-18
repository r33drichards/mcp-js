These packages are an isolated cargo-nix-plugin comparison; production outputs
and Cargo.lock are unchanged. Both comparison branches use Rust 1.93.1, the
plugin's locked modern nixpkgs, Nix 2.34, and dev debuginfo=line-tables-only.
The plugin itself is pinned to 00f311abab999b4ad4c7f3fa3d85bb17beb91aee.

Run the manual Nix build framework comparison workflow on this branch. Its
shared coordinator measures runtime bootstrap, plugin bootstrap, explicit
registry prefetch, artifact build, exact reuse, real test execution, and a
server source comment rebuild separately. No successful timings are claimed
until that workflow completes. The local environment has no Nix; syntax and
Python compilation checks are the available local validation.

The plugin's ABI requires its matching Nix 2.34 runtime. Build benchmark-nix,
benchmark-source-roots and benchmark-config first with ordinary Nix, then
benchmark-plugin with that runtime. Prefetch the registry with
benchmark-plugin/bin/cargo-nix-prefetch --manifest-path "$PWD/Cargo.toml".
Pass --option plugin-files benchmark-plugin/lib/nix/plugins when building
benchmark-tests and benchmark-runner. Execute
benchmark-runner/bin/run-benchmark-tests on every attempt, including cache hits.
The workflow handles durable GC roots and isolates CARGO_HOME.

The pinned upstream builder does discover binary unit harnesses despite its
README caveat. The adapter records the actual installed harness manifest and
compiles Cargo's compile-only node_compat_full example, which upstream omits.
It embeds a durable source path for CARGO_MANIFEST_DIR and preserves upstream
CARGO_BIN_EXE paths. Its rustdoc wrapper records the real doctest invocation
and required artifacts for uncached replay. The coordinator requires the
baseline's 81 harnesses, one example, and 82 actual result summaries, including
16 ignored doctests; missing coverage fails the comparison.

Feature parity remains unverified: the plugin resolves all workspace roots,
whereas baseline Cargo selects server. benchmark-config records this limitation;
benchmark-tests/share/benchmark-resolved-graph.json retains the resolver graph
for audit against Crane's Cargo feature report. Performance differences cannot
be attributed solely to the build framework until feature equivalence is
established. These adapters have not changed project manifests to suppress
that difference.

First CI attempt (run 44231511512) validated Nix/runtime and plugin bootstrap,
but failed the independent index check: upstream prefetch silently skipped
swc_core's 16.9 MB sparse index because ureq defaults to a 10 MB body limit.
The experiment patches only the prefetch utility to a bounded 64 MiB read;
that first fix preserved the plugin/resolver. The config records this patch and the
check remains mandatory. No server build or warm-cache result came from that
failed run. Its bootstrap measurements were runtime 43.275 s and plugin
189.475 s; these are integration diagnostics, not full benchmark results.

Second CI attempt (44232394638) passed prefetch and its independent check,
then the unmodified native resolver rejected the repository's existing
root [patch.crates-io] url path: upstream discovers member dependency path
edges but omits root patch paths. The trial adds an explicit resolver patch
that only seeds those path overrides into the existing source-discovery
queue. It includes a regression for a source-less local override with no
member path edge. Application manifests, lockfile and vendored url content
remain unchanged. benchmark-config records both upstream patches. The native
resolver also unifies resolver-2 host/target features as a conservative
superset; this is another reason feature parity remains unverified.
The root-path discovery regression passed with Rust 1.93.1; removing the
implementation while retaining the regression made it fail at the missing
patched-source assertion. Final result JSON is also printed in job logs so
measurements remain reviewable when artifact downloads are unavailable.

A local native-resolution audit with Rust/Cargo 1.93.1 found the same 595
server-reachable packages but different feature sets: the plugin additionally
enables reqwest 0.12.28 `blocking` and schemars 0.8.22 `chrono`/`uuid1`;
Cargo additionally reports chrono 0.4.44 `js-sys`/`wasm-bindgen`/`windows-link`.
The CI-produced resolver graph and Crane's build-time tree remain the final
comparison evidence. Timings for these workloads are diagnostic and cannot
establish a framework-only speed difference. No feature-semantics patch is
applied to conceal these differences.

The fourth run reached actual Rust test-target compilation, then exposed an
adapter source-filter omission: compat_surface embeds a repository-level
`tools/compat/surface_scan.js` file. The server's source closure now includes
root tools. An audit resolved all 77 literal include_str!/include_bytes! paths;
that scanner was the only path outside server, and none were missing. The
workflow also preserves valid intermediate Nix builds after a failed attempt
so later integration fixes can reuse completed dependencies; failed test
verdicts are never cached or treated as successful.
