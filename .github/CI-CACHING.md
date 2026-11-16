# Rust test CI caching

## Evidence and scope

In PR #322's cluster run [43810178836](https://github.com/r33drichards/mcp-js/actions/runs/43810178836),
engine-harness compilation took 3m20s and the six scenario tests took 6.82s.
The interrupted-publish test failed its convergence assertion; caching does not
fix that correctness failure. The non-V8 cluster command took about 41s including
compilation. The broader Test run
[43810178710](https://github.com/r33drichards/mcp-js/actions/runs/43810178710)
spent about five minutes in its separate server-build step before starting tests.
These are cold-build observations, not measured warm-cache improvements.

This change is based on main at `29c55599`. Main does not yet contain the separate
`server/engine-cluster-tests` workspace from the simulation stack. When that stack
lands, add a cache step for that workspace using the shared action, before its
existing test command:

```yaml
- uses: ./.github/actions/rust-cache
  with:
    shared-key: native-engine-cluster
    workspaces: server/engine-cluster-tests -> target
```

## What is cached

The shared action wraps a commit-pinned Swatinem/rust-cache. It caches Cargo
registry/git downloads, compiled dependency artifacts, and rusty_v8's archive
cache. It saves compiled dependencies even after test failures. It does not skip
tests, cache test results, or cache project test executables/incremental outputs.
Cargo checks fingerprints and recompiles invalid dependencies normally.

Native server workflows share `native-server`; standalone cluster/core/runner
workspaces use their own keys and target-directory mappings. Compiler version,
OS/architecture, manifests/lockfiles and relevant build environment contribute to
the action's automatic cache keys. Dependency changes can reuse a compatible
fallback and Cargo rebuilds what changed. Incompatible toolchains still miss.

The main Test workflow keeps Nix. Its Cargo cache identifies Rust through
`nix develop --command {0}`, with a distinct key including the Nix shell inputs,
so runner Rust and Nix Rust artifacts cannot accidentally share a cache. The
Nix store has its own commit-pinned cache action; its versioned key tracks
flake.lock, flake.nix and shell.nix. Before using the shell, `nix develop --profile`
creates a GC root under RUNNER_TEMP that lives through the cache action's post-job
save. CI verifies that the actual rustc and rusty_v8 store paths are in Nix's live
set. Without this profile, pre-save GC can delete the shell and Rust toolchain,
leaving a successfully uploaded but ineffective cache. The v2 key excludes those
older incomplete cache entries. The 3 GiB pre-save GC target is not a hard cache
size limit when the rooted shell closure exceeds it.

Caches are scoped by GitHub's branch/PR rules. The first run is cold; main runs
seed caches that PRs can restore. Caches created inside one PR are not generally
available to sibling PRs. Compatible parallel jobs may race to save the same
immutable key; the first save wins. This is safe but its cache may not contain
every dependency feature combination. A later run can still compile missing
artifacts. Cache eviction/restore/upload time must be included in measurements.

## Avoiding repeated work

- `cargo test -p server` already builds the application binary and test targets,
  so the main workflow no longer runs a separate `cargo build` first.
- Cargo prints captured output for failed tests. The workflow stores that output
  as an artifact instead of automatically executing the entire suite again.
  Bash pipefail preserves the test exit status through `tee`.
- The native HTTP/SQLite/cluster/runner workflows run feature branches through
  pull_request and main through push, avoiding duplicate push+PR builds.
- New commits cancel superseded PR runs within the same workflow/event/PR.
  Main pushes are not cancelled. Test commands, coverage and serial V8 settings
  are preserved. Scheduler-core retains its existing push-branch policy.
- Path-filtered workflows include shared-action changes, so edits to cache setup
  are actually validated by their consumers.

## Measuring after merge

Compare a cold run and a second run with unchanged toolchain/lockfiles: inspect
cache hit/fallback logs, compilation duration, actual test duration and cache
save/restore duration. Report observed differences instead of promising a speedup.
Only consider nextest/sharding or more test threads after execution time becomes
the bottleneck; multiple shards otherwise duplicate expensive compilation, and
existing V8/port-sensitive tests still need serialization. Nix package/VM builds
in other workflows need their own Nix-store/binary-cache work; a Cargo cache alone
does not speed them up.
