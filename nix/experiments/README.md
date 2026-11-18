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
the plugin/resolver remains unmodified. The config records this patch and the
check remains mandatory. No server build or warm-cache result came from that
failed run. Its bootstrap measurements were runtime 43.275 s and plugin
189.475 s; these are integration diagnostics, not full benchmark results.
