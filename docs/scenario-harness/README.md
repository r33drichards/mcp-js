# Declarative scenario harness: real JS on a real-sled cluster under simulation

Explainer for the #316 feature stack (PRs #312 -> #322 -> #336). Read the
diagrams in this directory first: stack-overview.svg (what talks to what),
scenario-lifecycle.svg (one scenario end to end), storage-ownership.svg
(who owns sled handles and how stale ones get fenced).

## The problem it solves

Before this stack, cluster behavior was tested with handwritten fault
injection: tests poked in-memory nodes directly and asserted on their
internals. That proves the mock behaves, not the system. This stack runs the
real thing — actual JavaScript executing on actual MCP-JS cluster nodes
backed by real sled databases, with network partitions, disk exhaustion, and
node crashes injected by a controller — and then checks meaningful
filesystem/database/cluster properties after recovery.

## One scenario, one runner, two oracles

A scenario is a serializable object (serde): seed, bounded topology (voters +
clients), a workload of timed RunJs operations (program -> write /data/... ->
snapshot -> publish CAS), timed faults (crash current leader, partition
groups, heal all), a recovery policy (stop workload, heal, restart crashed,
deadline), and explicit assertions. The same object drives three consumers:

1. Fixture tests: checked-in scenarios run deterministically and assert exact
   outcomes (known-answer regressions).
2. The fuzz track: libFuzzer generates structured scenarios; the modeled
   runner (in-memory nodes, the handwritten hooks we deliberately kept) is
   the fast oracle that explores topologies and fault schedules.
3. The real-I/O track (the `real-sled` feature): the identical scenario
   executes on Turmoil-simulated hosts whose `sled::Db` is the patched
   vendored sled with Turmoil's filesystem underneath. Only the controller
   may call Sim::crash/Sim::bounce; the observer and hosts exchange
   serialized RPC messages (ScenarioHost), never sled/engine handles.

## Why the real track matters

It is the proof that the production code paths — Raft appends, label CAS,
snapshot publication, console/blob writes — survive actual storage faults:
ENOSPC reaches sled's log flush, EIO reaches its config write, a crashed
host's database is dropped and reopened exclusively by a new generation
whose fenced handles make pre-crash writers fail closed. The modeled track
can assert "the oracle says X"; the real track demonstrates "the shipped
code did X under a real fs fault".

## Failure artifacts: replay and minimize

Each call to `Scenario::run_and_dump` writes three replay files:

- `<dir>/scenario.json` contains the exact scenario and seed.
- `<dir>/result.json` contains `status`, `acked`, `unknown`, `failed`, and `final_heads`.
- `<dir>/events.json` contains the full `ScenarioEvent` log.

Replay a failing fixture with:

    cargo test --manifest-path experiments/sled-fs/Cargo.toml --locked \
        --features scenario --test scenario_js <test_name> -- --nocapture

To turn a minimized scenario into a regression: reduce faults/workload to the
smallest failing case (drop one fault at a time while the assertion still
fails, shrink the topology to the minimum voters that still reproduce, then
shrink the JS program), copy the scenario into a `#[test]` calling
`run_and_dump` with the same assertion, and commit it. A failure must replay
without fuzzing; the minimized case is the regression.

## Evidence

Measured results and the hook-retirement audit live in
experiments/sled-fs/VALIDATION.md. Headline: scenario_js 13/13, cluster
suite green, storage/multi_node/recovery disk-full+eio suites green, patched
sled lib/log/tree 66/66, server lib 299/299, turmoil_engine real-sled 1/1.

