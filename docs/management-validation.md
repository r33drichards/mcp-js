# HTTP/MCP management validation

Base: `1eb74af4ace275bc803faf31d86c459433578495` (#295).
The test-first requirement arrived after implementation had started. The unchanged
new integration tests were then run against an isolated archive of that base to
establish the missing-interface failures. This is baseline regression evidence,
not a claim that the implementation was developed test-first.

## Reproduce the failing baseline

From the implementation checkout, with the repository Rust environment active:

```sh
mkdir -p /tmp/mcp-management-baseline
git archive 1eb74af4ace275bc803faf31d86c459433578495 | tar -x -C /tmp/mcp-management-baseline
cp server/tests/http_function_management_e2e.rs /tmp/mcp-management-baseline/server/tests/
cp scheduler-runner/tests/management.rs /tmp/mcp-management-baseline/scheduler-runner/tests/
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

## Passing local checks

Run from the implementation checkout:

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

Verified locally: nine registry unit tests; three registry integration tests;
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

The newer #284 fixes at `ffe1195` remain absent from this base. In particular,
management adapters do not authenticate the inherited Raft transport, add its
checkpoint bounds, or extend recovery to unresolved `Cancelling`. The new runner
listener is loopback-only; operator/recovery credentials are distinct from worker
and admission credentials. The stack still requires the integration work listed
in [feature status](../site-docs/reference/scheduler-http-status.md).
