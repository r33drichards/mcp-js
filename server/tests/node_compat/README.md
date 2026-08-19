# Node Compatibility Tests

This directory tracks two deliberately separate sources of Node compatibility
coverage:

- the executable fast suite uses upstream Node `v22.14.0` tests vendored under
  `vendor/`; and
- the broad inventory catalogs the complete test corpus from pinned
  `denoland/node_test`, currently containing Node `26.5.1` tests.

Do not combine their totals into one percentage. The Deno-vendored corpus is a
coverage backlog until a test is selected, adapted to the harness without
editing its source, and assigned an executable expectation.

## Commands

Run commands from the repository root:

```bash
tools/compat/node-compat.sh fetch
tools/compat/node-compat.sh inventory
tools/compat/node-compat.sh report
tools/compat/node-compat.sh fast
tools/compat/node-compat.sh family events
tools/compat/node-compat.sh profile pure
tools/compat/node-compat.sh check
```

Set `NODE_COMPAT_OFFLINE=1` to require already verified cached corpora. Downloads
live under `.cache/node-compat/` and are intentionally ignored by Git.

The underlying Rust filters are also available directly:

```bash
cd server
NODE_COMPAT_FILTER=event-emitter cargo test --test node_compat node_core_subset_matches_expectations -- --nocapture
NODE_COMPAT_FAMILY=events cargo test --test node_compat node_core_subset_matches_expectations -- --nocapture
NODE_COMPAT_PROFILE=pure cargo test --test node_compat node_core_subset_matches_expectations -- --nocapture
```

## Expectation Schema

Each fast-suite test in `expectations.json` records:

- `status`: `pass`, `fail`, `unsupported`, `harness_missing`,
  `policy_required`, or `flaky`;
- `family`: the Node module/API family;
- `profile`: the narrowest capability profile needed to exercise the contract;
- `compatibility`: `exact`, `adapted`, or `unsupported`;
- `reason`: required for every non-passing classification; and
- `expires`: required for temporary `flaky` classifications.

Runnable statuses are `pass` and `fail`. The fast suite skips classifications
that require a missing harness facility, an unavailable capability profile, or
an explicitly unsupported runtime facility.

## Capability Profiles

- `pure`: no host filesystem, subprocess, or network access.
- `filesystem`: isolated virtual filesystem and temporary-directory access.
- `subprocess`: allowlisted commands in a disposable environment.
- `network-client`: allowlisted outbound network operations.
- `network-server`: isolated listeners and loopback fixtures.
- `workers`: worker and message-channel facilities.
- `inspector`: reserved and unsupported initially.
- `native`: reserved while native addons and embedding remain out of scope.

Profiles are test configurations, not production defaults.

## Adding A Node Test

1. Pick an upstream test from `inventory.json` and identify its module family,
   fixtures, `test/common` dependencies, and narrowest capability profile.
2. Add the unmodified upstream source to the fast vendor list when it can run in
   normal pull-request CI.
3. Record it as `harness_missing`, `policy_required`, or `unsupported` with a
   concrete reason.
4. Add only the missing runner facilities needed by the selected upstream test.
   The runner must not emulate the product API under test.
5. Change the expectation to `fail` and run the focused test to prove the
   runtime behavior is missing.
6. Implement the runtime behavior in a separate module-family change.
7. Change the expectation to `pass`, rerun the family and fast suites, and
   regenerate reports.

This blocked-to-failing-to-passing sequence is required so compatibility work
remains test-first and expectation improvements are reviewable.

## Generated Files

After changing expectations or corpus pins, run:

```bash
tools/compat/node-compat.sh inventory
tools/compat/node-compat.sh report
```

Generated files:

- `inventory.json`: complete pinned Deno-corpus classification.
- `report.json`: machine-readable aggregate counts.
- `site-docs/reference/node-compatibility-status.md`: human-readable report.
- `site-docs/reference/compatibility.md`: existing compatibility overview.

CI rejects stale generated reports and any unexpected fast-suite result drift.

## Full Linux Corpus Workflow

`.github/workflows/node-compat-full.yml` attempts every inventoried Linux x86_64 test on 16 Railway shards. The aggregate `Node Compatibility Full` job is intentionally red until every applicable result passes; only explicit upstream non-Linux/non-x86_64 skips are neutral.
