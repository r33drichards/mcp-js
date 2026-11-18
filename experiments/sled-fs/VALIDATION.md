# Validation record

## Follow-up: 2026-11-18

The original 2026-11-15 record below remains a historical baseline. This
follow-up supersedes its negative namespace result: the opt-in Turmoil backend
now fsyncs newly written config and directory names at open. A new test first
failed on recovery (acknowledged value missing after crash) without fixture sync;
it passes after the adapter change. The std backend remains unchanged.

| Check | Observed result |
| --- | --- |
| Focused real-ClusterNode disk-full/crash/reopen/re-append test (seeds 7, 42, 314, two runs each) | 1 passed |
| Full isolated experiment with `cluster` | 8 passed (cluster 1, recovery 1, storage 6) |
| Default std-backend experiment | 1 passed |
| Upstream sled library, log and tree tests with `testing` | 102 passed, 1 ignored |
| `git apply --check` against published sled 0.34.7 and patched config comparison | Both succeeded |

The new workflow runs the isolated std and cluster suites on a pull request;
CI is not claimed green until its checks complete. Outstanding gaps remain:
JS scenario-runner integration, modeled lock ownership/host generations,
background worker scheduling, snapshot/blob crash ordering, and fsync-error
injection. No production storage dependency or default changed.

## Initial record: 2026-11-15

Executed 2026-11-15 in the isolated `poc/sled-turmoil-fs` checkout on x86_64 Linux,
Rust 1.100.0 (`0ef1bc386`, 2026-11-09). All commands ran from the MCP-JS root.
The environment had no Rust initially; the toolchain was installed separately at
`/workspace/rustup` and Cargo at `/workspace/cargo`. This was the command prefix:

```sh
export RUSTUP_HOME=/workspace/rustup
export CARGO_HOME=/workspace/cargo
export PATH=/workspace/cargo/bin:$PATH
export RUSTFLAGS='--cap-lints warn'
```

| Command | Result |
| --- | --- |
| `cargo test --manifest-path experiments/sled-fs/Cargo.toml --locked` | **1 passed**, std backend roundtrip, blob and lock exclusion. |
| `cargo test --manifest-path experiments/sled-fs/Cargo.toml --features cluster --locked -- --test-threads=1` | **7 passed**: 1 actual cluster test, 6 storage/config tests. Includes three seeds for host isolation and nine crash/flush/namespace combinations; pending blob crash test uses seed 314. |
| `cargo test --manifest-path experiments/sled-fs/Cargo.toml -p sled --features testing --lib --test test_log --test test_tree -- --test-threads=1` | **102 passed, 1 pre-existing ignored**: 30 unit tests (2.13s), 6 log tests (1.50s), 66 tree tests (162.74s). The ignored `log_writebatch` was already marked upstream as hanging; unchanged. |
| `cargo test --manifest-path server/cluster-tests/Cargo.toml --locked -- --test-threads=1` | **42 passed**: 35 library regressions, seeded fuzz integration, HTTP network integration, 5 peer tests. Original production dependency selection, no local override. |
| `patch --dry-run -p1 -d /workspace/sled-sources/sled-0.34.7 < experiments/sled-fs/adapter.patch` | All 13 changed/new files apply cleanly against published 0.34.7. |
| `diff -qr /workspace/sled-sources/sled-git/src /workspace/sled-sources/sled-0.34.7/src` | No differences: cloned release commit matches published crate source. |

The upstream suite command did not use `--locked` on its first dependency
resolution; its resolved dependencies are now in the committed experiment
lockfile. The README uses `--locked` for subsequent reproducible runs.

## Initial failures and fixes

- First compilation without the lint cap failed on 42 old
  `unused_qualifications` denials. Capping lints matches how registry dependencies
  are normally treated; the vendored source's lint policy was not rewritten.
- First fault assertions expected OS errno 5/28. Inspection verified Turmoil
  0.7.2 uses `Error::other` strings instead. Assertions now check the actual
  propagated error, with fault-off controls; no adapter error was fabricated.
- First attempt at upstream integration tests used `compression` only; upstream
  explicitly requires `testing`. Retried with that feature.
- The published upstream tree test had an immutable Subscriber binding while
  calling `next_timeout(&mut self)`. The one-line `let mut sub` repair is in the
  review patch. The complete tree suite then passed; no test was removed.

## Not run / not established

- Full MCP-JS/V8 builds, JS scenarios, coverage-guided fuzzing, sanitizers,
  cross-platform checks, release-mode/performance tests, and upstream sled's
  separate subprocess crash, space-leak, quiescent, and failpoint integration
  binaries were not run. Existing files remain intact.
- No full snapshot/rename/delete fault matrix, torn-write/corruption campaign,
  simulated background worker/flusher execution, fsync-error injection, modeled
  lock contention, stale-handle isolation, or cross-host handle migration.
- No claim that newly created database namespaces or flushed blob contents
  survive arbitrary power failure. The negative namespace test demonstrates
  an actual durability gap under this simulator's strict directory model.
- No merge, deployment, upstream sled PR, or changes to production defaults.
