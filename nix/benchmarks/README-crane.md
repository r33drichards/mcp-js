# Crane comparison branch

Experimental branch `perf/crane-comparison`, based on `785790ad` so the production
server and test suite match the cargo-nix-plugin experiment. Existing production
packages and the default dev shell are unchanged.

## Pinned inputs and equivalent environment

- Crane: `95e62def851e9d650f0eab63cb4f59a3306e20bd`.
- Common platform source: cargo-nix-plugin
  `00f311abab999b4ad4c7f3fa3d85bb17beb91aee`, used only to access its locked modern
  nixpkgs and Nix 2.34 runtime. Crane does not use the Cargo resolver plugin.
- Shared nixpkgs: `ed67bc86e84e51d4a88e73c7fd36006dc876476f`.
- Rust: the project's existing locked rust-overlay toolchain (1.93.1), not modern
  nixpkgs' default Rust. `[profile.dev] debug = "line-tables-only"` and opt-level0
  remain authoritative; no release profile is substituted.
- The root `Cargo.lock`, local URL fork, pre-fetched V8 archive, LLVM/native build
  inputs and OpenSSL are retained. Common modern nixpkgs intentionally differs
  from the original main dev shell, but is shared by both experimental branches.

## Build/run interface

- `.#benchmark-nix`: matching Nix 2.34 used for both experiments.
- `.#benchmark-source-roots`: source-path marker to retain evaluation inputs.
- `.#benchmark-config`: JSON compiler/lockfile/upstream provenance.
- `.#benchmark-deps`: Crane's dummy-source dependency/test-dependency artifacts.
- `.#benchmark-tests`: compilation only, with installed unit/bin/integration test
  executables, compile-only example, full target artifacts and captured rustdoc
  command. `share/benchmark-targets.json` inventories what was actually built.
- `.#benchmark-runner/bin/run-benchmark-tests`: uncached process execution of the
  discovered tests and doctests, sequentially with the same ignored-test policy.
  Nix never caches this runner's test verdict.

Crane's dummy source preserves the real patched URL subtree. Replacing URL with
an empty dummy library would break registry dependencies such as reqwest that
import its public API. The full server fixture source is not a dependency-artifact
input; project-only changes must leave `benchmark-deps` unchanged.

The test artifact checks its discovered Cargo targets against the baseline
inventory in `scripts/ci/expected-nix-comparison-tests.json`: one library, five binary-unit targets,
75 integration targets and one compile-only example. Its rustc wrapper embeds
immutable fixture and installed binary paths instead of `/build/source` paths.
The doctest capture records Cargo's exact rustdoc arguments and environment but
runs no tests during artifact construction. The external runner replays real
rustdoc even though the baseline's sixteen doctests are ignored. Every target
and the real rustdoc invocation must succeed on every attempt. The historical
860 passed / 32 ignored counts are recorded for workload comparisons; adding
tests changes the comparison flag instead of failing an otherwise successful
run. Missing target summaries and test failures still fail the job.

`share/cargo-features.txt` records Cargo's selected `-p server` feature graph for
comparison with the native plugin resolver. Any feature-selection difference,
missing target, build failure or mismatched runtime result makes timings
non-equivalent; it must be reported rather than suppressed.

## Validation status

The first complete artifact build succeeded in CI run `44232928137`: 1484.117s
for compilation and 3.020s for exact warm lookup with identical output paths.
All 81 executable targets ran successfully: 860 passed, 16 ignored. The real
rustdoc invocation then failed because capture omitted `CARGO_CRATE_NAME`, which
UniFFI macros require. The adapter now retains Cargo's complete `CARGO_*`
environment and relocates inherited dependency build-script target paths.
The subsequent complete run `44236828144` passed 860 tests with 32 ignored,
including real rustdoc execution. Its source-only rebuild took 460.792s and
reused dependency artifacts.

Python syntax, wrapper path-rewrite compile/run checks, doctest-command relocation
and an 81-target/one-example installation fixture were checked locally. The local
host has no Nix installation, so those checks do not establish that the Nix build
or the 860-test runtime is green. Actual CI evaluation/build/run results are the
source of truth for the comparison.
