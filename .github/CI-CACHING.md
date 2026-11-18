# Shared CI environments and caching

## Measurements

The native SQLite job improved from **5m39s to 2m59s** on the same commit
([cold](https://github.com/r33drichards/mcp-js/actions/runs/43811816228/attempts/1),
[warm](https://github.com/r33drichards/mcp-js/actions/runs/43811816228/attempts/2)).
Both runs executed the same ten passing tests; queue time is excluded.

The Nix comparison was **10m40s versus 10m34s**, effectively unchanged
([baseline](https://github.com/r33drichards/mcp-js/actions/runs/43814318323/attempts/1),
[rerun](https://github.com/r33drichards/mcp-js/actions/runs/43814318323/attempts/2)).
The rerun found the cache but failed to extract root-owned `/nix/store` paths
with permission errors, then downloaded dependencies again. A cache lookup hit
is not proof of a successful restore. These measurements precede the shared
single-user Nix setup below; no new performance improvement is claimed yet.

The general Nix suite also spends substantial time executing tests: its baseline
compiled for 2m52s, then ran tests for more than five minutes, including 102s in
`wpt_subset_matches_expectations`. Caching cannot remove that execution time.

## Native Rust jobs

Use `setup-rust` after checkout. It installs the pinned toolchain action and
calls the shared dependency-cache action; each job declares only its build family,
workspace mapping and optional lint components:

```yaml
- uses: ./.github/actions/setup-rust
  with:
    shared-key: native-server
    components: rustfmt
```

Root server jobs share `native-server`. Independent cluster/core/runner workspaces
use separate keys and target mappings. For the engine workspace on the simulation
stack (not yet on this main baseline), use:

```yaml
- uses: ./.github/actions/setup-rust
  with:
    shared-key: native-engine-cluster
    workspaces: server/engine-cluster-tests -> target
```

## Nix Rust jobs

Use `setup-nix-shell` after checkout, as the Test and CLI workflows do:

```yaml
- uses: ./.github/actions/setup-nix-shell
- run: nix develop "$MCP_CI_NIX_PROFILE" --command cargo test --locked -p server -- --test-threads=1
```

This action owns the installer, store cache, GC roots, verification, and Cargo
cache. It installs a runner-owned single-user Nix store using the installer
recommended by cache-nix-action, so restoring store archives does not require
writing root-owned paths. A found-but-not-restored cache fails explicitly.

`prepare.sh` roots both the development environment and recursively archived
flake sources; pre-save GC must retain the Rust compiler, V8 archive and flake
inputs. Keeping only a shell profile can still discard the sources needed to
evaluate its flake. The live-path check covers both sets. The v3 cache namespace
excludes the older root-owned/incomplete entries. Cache keys track OS/architecture,
flake/shell inputs and the setup implementation. The pre-save 3 GiB GC target is
not a hard size limit if live roots exceed it.

The prepared profile is exported once as `MCP_CI_NIX_PROFILE` and is also an action
output. Subsequent Cargo cache probes, builds and test commands enter that profile
directly, avoiding repeated flake evaluation. Nix officially supports reusing a
recorded environment with `nix develop /path/to/profile`. Debug server jobs and
release CLI jobs use separate Cargo cache families; native Rust artifacts never
share a key with Nix Rust artifacts. The CLI builds only the two required binaries
in one Cargo invocation, rather than also building the server's docs generators.

The store-cache action's post-job save runs while the profile/source roots are
still alive. Never delete those roots in an earlier cleanup step.

## Cache and test boundaries

The underlying commit-pinned Rust cache stores Cargo registry/git downloads,
compiled dependencies and rusty_v8 archives, including after test failures. It
does not cache test results or project test executables/incremental outputs.
Cargo still validates fingerprints and compiles changed or missing dependencies.

GitHub cache scope/eviction rules apply. Main runs seed caches available to PRs;
a PR's cache is not generally available to sibling PRs. Parallel compatible jobs
may race for an immutable cache key: the first save wins and may contain fewer
feature combinations. Missing artifacts still compile normally. The first run
for a new namespace is cold. Include restore/upload overhead in comparisons.

Native path-filtered workflows validate shared-action changes. Superseded PR
runs are cancelled, and feature branches use PR checks instead of duplicate
push+PR runs where configured. Main pushes remain independent. Test commands and
serial V8 settings are retained. The main workflow records failed-test output
instead of automatically rerunning the entire suite; Bash pipefail preserves the
failure status through `tee`.

Pure Nix package/VM jobs are outside this action's scope: they need their own
rooted output/binary-cache setup, not an unnecessary Rust development shell.
Measure the shared setup on cold and warm runs before claiming a speedup. Consider
safe test grouping/sharding separately for the remaining execution time.
