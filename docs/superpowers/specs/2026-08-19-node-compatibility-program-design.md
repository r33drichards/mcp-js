# Node Compatibility Program Design

Date: 2026-08-19

## Summary

Build a long-running Node.js compatibility program for `mcp-v8` that combines
three complementary sources of evidence:

1. upstream Node.js core tests, expanded module family by module family;
2. the complete Node test corpus vendored by `denoland/node_test`; and
3. differential and ecosystem testing against Node.js and Node CITGM.

The target is the highest practical Node.js compatibility achievable without
embedding Node. Compatibility must remain explicit about capability boundaries:
passing a Node test must not silently expose host filesystem, process, network,
or native-addon access. Tests that require host effects run only under named,
policy-gated capability profiles.

The program extends the existing Node compatibility harness rather than
creating a parallel conformance framework. The current fast suite remains the
per-PR baseline while broader corpora and ecosystem tests run in filtered,
scheduled, or nightly jobs.

## Goals

- Make upstream Node.js tests the primary executable specification for Node
  behavior.
- Adopt the Deno-vendored Node test corpus as a broad, pinned source corpus.
- Preserve upstream test sources without compatibility-specific edits.
- Classify every attempted test with a reviewable expectation and reason.
- Support explicit capability profiles for host-coupled APIs.
- Add deterministic differential tests against a pinned Node executable.
- Add curated ecosystem coverage sourced from Node CITGM.
- Publish compatibility results by module, API family, and capability profile.
- Ratchet improvements and regressions through CI expectation drift.
- Keep the default web-compatible runtime and security posture unchanged.

## Non-Goals

- Embed Node.js or link against `libnode`.
- Claim complete Node compatibility from a single aggregate percentage.
- Support native `.node` addons, Node-API, V8 ABI addons, or `node-gyp` in the
  initial program.
- Make unrestricted host filesystem, subprocess, or network access available
  merely to satisfy upstream tests.
- Modify upstream Node tests to match `mcp-v8` behavior.
- Run all Node and CITGM tests on every pull request.
- Treat differential output equivalence as more authoritative than documented
  Node API behavior and upstream core tests.

## Compatibility Definition

`mcp-v8` compatibility is defined as Node-compatible behavior under a declared
runtime profile. A profile states which host capabilities are available and
which sandbox substitutions are intentional.

Compatibility results use three behavior levels:

- `exact`: behavior matches the pinned Node release for the covered test.
- `adapted`: behavior is intentionally sandboxed but preserves the documented
  API contract relevant to portable applications.
- `unsupported`: the behavior requires an excluded facility or has not yet
  been implemented.

An adapted result must include a reason. Examples include synthetic process
identity, policy-gated network access, and virtual filesystem paths.

## Version Pins

The program pins every external oracle:

- Node.js release tag used by upstream tests and the differential executable.
- `denoland/node_test` commit containing its vendored Node test tree.
- Node CITGM commit and package manifest revision.
- Any package manager or bundler used by ecosystem probes.

The initial Node target remains `v22.14.0`, matching the existing compatibility
suite. Version updates are explicit pull requests that regenerate inventories,
run the broad suites, and review all expectation drift.

## Track A: Upstream Node Core Tests

### Existing Fast Suite

Keep `server/tests/node_compat.rs` and its currently vendored subset as the
fast per-PR conformance suite. Existing passing results remain the baseline.

The harness continues to:

- execute each test in a fresh isolate;
- provide a CommonJS-like test shell and supported `test/common` helpers;
- capture structured results under a sentinel;
- compare results against committed expectations; and
- fail on both regressions and unexpected improvements.

### Full Corpus Source

Add tooling that downloads a pinned `denoland/node_test` archive into a cache or
ignored workspace directory. The Deno repository is used as a convenient,
complete, license-preserving vendor of Node's `test/` directory, not as the
runtime or test executor.

The repository does not initially commit the approximately seven thousand
JavaScript tests. Instead it commits:

- the source pin;
- a checksum;
- the downloader/update command;
- a generated inventory; and
- the expectation and classification manifests.

CI caches the downloaded corpus by commit and checksum.

### Inventory

Generate an inventory for relevant Node test directories, initially including:

- `test/parallel`;
- `test/sequential`;
- `test/es-module`;
- `test/fixtures` dependencies referenced by selected tests; and
- `test/common` dependencies needed by the runner.

Directories that fundamentally depend on embedding, native binaries, or Node's
own build artifacts are inventoried but excluded from execution profiles. These
include native addon, C++ embedding, inspector integration, and Node executable
packaging suites.

Each inventory entry records:

- upstream path;
- inferred module family;
- required fixtures and `test/common` helpers;
- declared or inferred capability profile;
- current expectation;
- compatibility level;
- reason and tracking issue when not passing; and
- last observed Node and corpus versions.

### Expectation Schema

Expand the current expectation model to support:

- `pass`: expected to complete successfully;
- `fail`: runnable and expected to fail with recorded diagnostics;
- `unsupported`: blocked by an intentionally excluded runtime facility;
- `harness_missing`: blocked by missing test-runner support rather than runtime
  behavior;
- `policy_required`: runnable only under one or more capability profiles; and
- `flaky`: temporarily quarantined with an owner, reason, and expiration.

Every non-pass state requires a reason. `flaky` additionally requires an expiry
date so quarantines cannot become permanent silently.

Expectation updates are generated but reviewed. CI rejects uncommitted drift in
either direction.

### Capability Profiles

Tests run under the narrowest profile capable of exercising their contract:

- `pure`: no host filesystem, subprocess, or network operations;
- `filesystem`: isolated virtual filesystem and temporary-directory access;
- `subprocess`: allowlisted commands in a disposable execution environment;
- `network-client`: allowlisted outbound DNS, TCP, TLS, HTTP, HTTP/2, and UDP;
- `network-server`: isolated listeners and loopback fixtures;
- `workers`: worker and message-channel execution facilities;
- `inspector`: reserved and unsupported initially; and
- `native`: reserved and unsupported while embedding and native addons remain
  out of scope.

Profiles are test configuration, not production defaults. Each profile maps to
explicit `mcp-v8` policies and ephemeral fixtures.

### Runner Growth

Extend the runner only in response to selected upstream tests. Runner additions
may provide:

- more of Node's `test/common` contract;
- fixture resolution;
- temporary directories;
- CommonJS wrapping and module metadata;
- process lifecycle and exit observation;
- controlled loopback servers;
- child-process fixtures; and
- structured skip and platform predicates.

Runner code must not emulate the API under test. It may provide test harness
facilities, but product behavior belongs in the runtime implementation.

## Track B: Differential Testing

Add a separate differential harness that executes the same fixture under:

1. the pinned Node.js executable; and
2. `mcp-v8` under a declared capability profile.

Fixtures return a structured JSON report containing relevant values, thrown
errors, event order, stdout, stderr, and termination state. The harness compares
normalized reports.

Normalization is explicit and narrowly scoped. Permitted normalization includes
values that are intentionally environment-specific:

- process identifiers;
- temporary paths;
- platform and architecture labels where adapted behavior is documented;
- timestamps and bounded timing tolerances;
- memory counters; and
- nondeterministic port allocation.

Differential tests do not compare arbitrary console snapshots when a structured
assertion is possible. Timing-sensitive and scheduling-sensitive tests require
event-order assertions rather than exact durations.

Differential coverage is used for:

- regression tests derived from package incompatibilities;
- API edge cases not isolated by upstream Node tests;
- adapted sandbox behavior with a documented normalization; and
- fast validation during implementation before adding a broader upstream test
  family.

## Track C: Ecosystem And CITGM

Pin Node CITGM and import its package manifest as the ecosystem candidate list.
CITGM itself assumes a functioning Node executable, npm installation, local
filesystem, and subprocess support, so its runner cannot initially execute
inside `mcp-v8` unchanged.

Adoption proceeds in stages:

### Stage 1: Package Load Probes

- Select pure-JavaScript packages without native install steps.
- Download and install packages with trusted host tooling in disposable
  directories.
- Resolve or bundle a declared entrypoint.
- Execute a deterministic load probe inside `mcp-v8`.
- Record whether the package loads and exposes its expected top-level API.

### Stage 2: Basic API Probes

- Add package-specific deterministic scenarios that exercise representative
  public APIs.
- Prefer upstream examples or smoke tests over custom synthetic behavior.
- Run each probe under its declared capability profile.

### Stage 3: Upstream Package Tests

- Run package-owned test commands only after CommonJS resolution, filesystem,
  subprocess, Node CLI, and package-manager assumptions are sufficiently
  compatible.
- Preserve package test sources and configuration.
- Record environment adaptations and skipped sub-suites explicitly.

Package results use independent levels:

- `installs`;
- `loads`;
- `basic_api`;
- `upstream_tests_partial`; and
- `upstream_tests_full`.

Native-addon packages remain visible in reports but are classified unsupported
until a separately approved non-embedding strategy exists.

## Module-Family Roadmap

Implementation proceeds in dependency order:

1. runtime globals, CommonJS wrapping, package resolution, and `node:module`;
2. buffer, process, events, streams, util, console, timers, and crypto;
3. filesystem, path, OS, temporary directories, and fixture semantics;
4. subprocess imports, spawn/exec/fork contracts, stdio, and lifecycle;
5. DNS, TCP, UDP, TLS, HTTP, HTTPS, HTTP/2, and controlled servers;
6. workers, VM contexts, async hooks, diagnostics, and performance APIs;
7. Node CLI flags, loaders, package conditions, and test-runner compatibility;
8. broader CITGM package tests and application-level compatibility.

Each module-family effort begins by selecting upstream tests and recording them
as failing or blocked before product implementation starts.

## Reporting

Generate machine-readable JSON and human-readable Markdown reports containing:

- totals by expectation state;
- totals by module family;
- totals by capability profile;
- exact versus adapted compatibility;
- changes from the previous committed baseline;
- top harness blockers;
- top runtime blockers; and
- ecosystem package levels.

Reports must avoid a single headline percentage without its denominator and
profile. A valid summary states, for example, the passing percentage of selected
`node:stream` tests under the `pure` profile for Node `v22.14.0`.

## CI Strategy

Use layered CI to keep feedback practical:

- `node-compat-fast`: existing curated tests on every relevant pull request;
- `node-compat-family`: changed module families and their dependencies;
- `node-compat-differential`: deterministic differential fixtures;
- `node-compat-full`: all currently runnable upstream tests, scheduled nightly;
- `node-compat-ecosystem`: curated package probes, scheduled or manually
  dispatched; and
- `node-compat-version-drift`: periodic check against newer Node, Deno corpus,
  and CITGM revisions without automatically changing pins.

Docs-only changes continue to skip heavy compatibility jobs where existing CI
rules permit.

## Security Requirements

- The default runtime remains web-compatible and least-capability.
- Capability profiles are explicit and test-specific.
- Test fixtures cannot read host credentials or arbitrary host paths.
- Network fixtures use isolated loopback or allowlisted destinations.
- Subprocess tests use allowlists and disposable environments.
- Header and credential injection remain host-side and unreadable from the
  isolate.
- A passing Node test cannot justify exposing unrestricted sockets, files,
  environment variables, or process controls.
- Compatibility reports distinguish exact behavior from security adaptations.

## First Pull Request

The foundational pull request changes test infrastructure and documentation,
not runtime APIs. It will:

- add the expanded expectation data model;
- add pinned `denoland/node_test` source metadata and checksum validation;
- add corpus download/update tooling using a cache or ignored directory;
- generate a complete test inventory without running every test;
- migrate the existing 37-test baseline without changing outcomes;
- add module-family and capability-profile classification;
- generate JSON and Markdown compatibility reports;
- add commands for fast, filtered, and future full-suite execution; and
- document how contributors move a test from blocked to failing to passing.

The first pull request must preserve the existing 35 passing and two ignored
test outcomes. Runtime behavior changes begin in subsequent module-family pull
requests using test-first development.

## Follow-Up Pull Requests

Expected follow-ups include:

1. the first module-family runner extensions and failing upstream tests;
2. globals, CommonJS, package-resolution, and `node:module` tests;
3. pure-JavaScript builtin module families;
4. filesystem and process capability profiles;
5. networking profiles and loopback fixtures;
6. differential runner and initial fixtures;
7. CITGM manifest synchronization and load probes; and
8. scheduled full-corpus and ecosystem CI.

Individual pull requests remain reviewable and independently testable even
though they contribute to the larger program.

## Success Criteria

The foundational work succeeds when:

- external versions and source corpora are reproducibly pinned;
- the complete corpus can be inventoried from a clean checkout;
- every inventoried test can receive a module family, profile, and expectation;
- existing fast-suite results remain unchanged;
- expectation and report drift fail CI;
- contributors can run one test, one module family, or the fast suite locally;
- the generated report identifies the next highest-value compatibility gaps;
  and
- the program can add differential and ecosystem results without replacing the
  Node-core source of truth.
