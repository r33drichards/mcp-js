# HTTP/MCP management validation

Historical base: `1eb74af4ace275bc803faf31d86c459433578495` (#295).
Historical implementation: `1d395631` (#299 before stack hardening integration).
The observations below belong to those revisions, not the newly integrated stack.
The test-first requirement arrived after implementation had started. The unchanged
new integration tests were then run against an isolated archive of that base to
establish the missing-interface failures. This is baseline regression evidence,
not a claim that the implementation was developed test-first.

## Reproduce the failing baseline

Use the historical test files, with the repository Rust environment active. Do
not copy the current integrated tests: their required cluster configuration and
assertions have changed.

```sh
mkdir -p /tmp/mcp-management-baseline
git archive 1eb74af4ace275bc803faf31d86c459433578495 | tar -x -C /tmp/mcp-management-baseline
git show 1d395631:server/tests/http_function_management_e2e.rs > /tmp/mcp-management-baseline/server/tests/http_function_management_e2e.rs
git show 1d395631:scheduler-runner/tests/management.rs > /tmp/mcp-management-baseline/scheduler-runner/tests/management.rs
```

The server check used the shared target cache. Touching source timestamps forces
Cargo to rebuild the archived sources instead of reusing a newer binary:

```sh
cd /tmp/mcp-management-baseline
touch server/src/lib.rs server/src/main.rs server/src/mcp.rs server/src/http_functions.rs server/src/engine/mod.rs server/tests/http_function_management_e2e.rs
CARGO_TARGET_DIR=/workspace/mcp-js/target cargo test --locked -p server --test http_function_management_e2e http_and_mcp_share_revisions_validation_and_invocation -- --exact --nocapture
CARGO_TARGET_DIR=/workspace/mcp-js/scheduler-runner/target cargo test --locked --manifest-path scheduler-runner/Cargo.toml --test management real_http_mcp_auth_validation_and_management
```

Observed failures, with both logs confirming compilation from the baseline path:

- Server: `http_and_mcp_share_revisions_validation_and_invocation` fails at the
  assertion that real MCP `tools/list` contains the six management tools.
  Result: **0 passed, 1 failed**. The baseline lists only its existing tools.
- Scheduler: startup rejects `management` as an unknown configuration field;
  `real_http_mcp_auth_validation_and_management` fails waiting for acquired owner.
  Result: **0 passed, 1 failed**.

No production files in the archive were edited. The implementation checkout and
all parent branches remained separate. Restore the implementation binaries before
running the passing checks below when sharing target directories.

## Historical passing local checks

These commands and results describe the historical implementation checkout; rerun
them against the final integrated commit before claiming current validation:

```sh
cargo check --locked -p server --bin server
cargo build --locked -p server --bin server
cargo test --locked -p server --lib http_functions -- --test-threads=1
cargo test --locked -p server --test http_function_management_e2e --test jwks_auth_e2e -- --test-threads=1
cargo test --locked -p server --lib scheduled_runtime_tests -- --test-threads=1
cargo test --locked --manifest-path scheduler-runner/Cargo.toml
cargo clippy --locked --manifest-path scheduler-runner/Cargo.toml --all-targets -- -D warnings
cargo fmt --manifest-path scheduler-runner/Cargo.toml --check
rustfmt --edition 2024 --check server/src/http_functions.rs server/tests/http_function_management_e2e.rs server/tests/jwks_auth_e2e.rs
python3 -m mkdocs build --strict
```

Historically verified locally: nine registry unit tests; three registry integration tests;
three JWKS integration tests; the actual Engine scheduled-runtime test; 40 scheduler tests (including both new management integration tests); scheduler Clippy; strict MkDocs; 7,040
local documentation links/fragments; and 31 JSON examples in changed docs.

The tests cover shared HTTP/MCP CAS races, stale revisions and delete/recreate,
invalid arguments, binary invocation, both server MCP modes, disabled registries,
missing/invalid credentials, dedicated-worker route exclusion, scheduler operator
and recovery separation, cancellation after disablement, and retained uncertainty.
No assertions were removed or tests disabled to obtain these results.

## CI and integration limits

The draft PR enables Docs Check for the #295 base and includes the new server test
in Dynamic HTTP functions CI. Existing scheduler CI discovers the new tests.
Affected NixOS tests exercise both transports in the existing single-node,
cluster, disable, cancel, and disable-then-cancel scenarios. This local environment
has neither Nix nor `/dev/kvm`; VM results must be verified in remote CI.

## Integrated stack validation boundary

The #284 fixes at `ffe1195` are now integrated through #287, #293, #295, and
#299. Fixed-voter Raft authentication, checkpoint bounds, and attested recovery
for unresolved `Cancelling` come from that runtime hardening, not the management
adapters. Runner configurations now require `cluster_bearer_env` even for a
single voter. The management listener remains loopback-only; operator/recovery
credentials must differ from worker, admission, and cluster credentials.

Fresh combined-stack validation is separate from the historical counts above.
Until a final integrated commit and its test results are recorded, do not treat
those counts as current passes or imply NixOS VM, remote CI, deployment, or
main-merge success. See [feature status](../site-docs/reference/scheduler-http-status.md).
