# Validate builds and filesystem performance

Use source and runtime validation separately: a cached executable is not a cached
test verdict, and a benchmark improvement is not a compatibility guarantee.

## Reuse Crane compilation artifacts

The [Crane comparison pipeline](https://github.com/r33drichards/mcp-js/blob/main/nix/benchmarks/README-crane.md)
builds test executables and records Cargo's target inventory. Its external runner
executes every installed test target and the real rustdoc invocation on each run,
even when Nix restores the compilation outputs from cache.

The comparison checks exact warm reuse and source-only invalidation. It records
artifact sizes, timings, feature selection, target coverage, and runtime results.
Historical test totals are a comparison baseline, not a reason to skip new tests;
when adding/removing a target, update
`scripts/ci/expected-nix-comparison-tests.json` to match Cargo metadata.

```bash
python3 scripts/ci/compare-nix-builds.py --implementation crane \
  --results /tmp/mcp-build-comparison
```

Run in the configured Nix environment with enough disk for V8-linked artifacts.
The CI workflow retains the phase logs and comparison JSON. Avoid publishing
speedup percentages without comparable cold/warm measurements.

## Exercise scheduler and consensus contracts

```bash
cargo test --locked --manifest-path server/cluster-tests/Cargo.toml
cargo test --locked --manifest-path scheduler-core/Cargo.toml
cargo test --locked --manifest-path scheduler-runner/Cargo.toml
```

The [model replay instructions](https://github.com/r33drichards/mcp-js/blob/main/specs/runner/README.md)
cover the checksum-pinned TLC tool, generated traces, and disposable source
mutations. Keep expected counterexamples distinct from unexpected safety
failures. Local TCP integration, bounded model exploration, and real-storage
restart tests cover different boundaries; none alone proves arbitrary
network/storage fault behavior or exactly-once external effects.

## Reproduce the filesystem workload

The [filesystem grep benchmark](https://github.com/r33drichards/mcp-js/blob/main/loadtest/fs-grep.md)
compares local and S3-backed snapshot scans, with cold/warm passes, corpus
revisions, count checks, and distinct request-to-completion versus guest scan
timings. A local S3 emulator can check correctness; an intended production endpoint
is needed for representative storage latency.

```bash
node loadtest/fs-grep.mjs --s3-bucket YOUR_BENCHMARK_BUCKET \
  --mdn-files 100 --wiki-shards 1 --wiki-articles 1000
```

This command uses credentials and writes to the named bucket. Use a dedicated
benchmark bucket and follow the benchmark's cleanup/retention instructions.
The full workload downloads large corpora and leaves S3 snapshot objects.

The opt-in `.auto/` research setup pins/prepares corpora, checks expected counts,
records a baseline before candidates, and runs filesystem regressions. Read
[the measurement contract](https://github.com/r33drichards/mcp-js/blob/main/.auto/prompt.md)
and [references](https://github.com/r33drichards/mcp-js/blob/main/.auto/references.md)
before running `.auto/measure.sh`. Keep workloads, timing boundaries, and
correctness oracles unchanged while comparing candidates. The experiment cap is
33 measurements; retain verified results and regressions rather than assuming a
candidate is faster. This is developer tooling, not a runtime scheduler feature.

## Source

- [Comparison driver](https://github.com/r33drichards/mcp-js/blob/main/scripts/ci/compare-nix-builds.py)
- [Filesystem prefetch](https://github.com/r33drichards/mcp-js/blob/main/server/src/engine/fs_store.rs)
- [Research checks](https://github.com/r33drichards/mcp-js/blob/main/.auto/checks.sh)
