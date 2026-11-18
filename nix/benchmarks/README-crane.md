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
inventory in `expected-tests.json`: one library, five binary-unit targets,
75 integration targets and one compile-only example. Its rustc wrapper embeds
immutable fixture and installed binary paths instead of `/build/source` paths.
The doctest capture records Cargo's exact rustdoc arguments and environment but
runs no tests during artifact construction. The external runner replays real
rustdoc even though the baseline's sixteen doctests are ignored. Thus a warm
artifact lookup must still produce 82 real test-result summaries, with the
baseline 860 passed and 32 ignored, before claiming test parity.

`share/cargo-features.txt` records Cargo's selected `-p server` feature graph for
comparison with the native plugin resolver. Any feature-selection difference,
missing target, build failure or mismatched runtime result makes timings
non-equivalent; it must be reported rather than suppressed.

## Validation status

Python syntax, wrapper path-rewrite compile/run checks, doctest-command relocation
and an 81-target/one-example installation fixture were checked locally. The local
host has no Nix installation, so those checks do not establish that the Nix build
or the 860-test runtime is green. Actual CI evaluation/build/run results are the
source of truth for the comparison.
