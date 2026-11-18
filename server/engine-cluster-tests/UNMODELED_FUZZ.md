# Broader JavaScript cluster fuzzing (#316, milestone 4)

fuzz_js_unmodeled maps libFuzzer input bytes into a bounded JavaScript
program, seed, and short partition/heal schedule. The same serialized scenario
runs in fixtures, fuzzing, and replay. Unlike modelable fuzz_js_scenario, it
does not compare arbitrary JS output or filesystem contents to a reference
model. It checks unexpected engine failures, acknowledged heads in the legal
history, and replica convergence after recovery. An ordinary JS exception
within the generated pure-JS body is caught and tagged through the reserved
__mcp_http_result module export; a bare async IIFE return is not part of
Engine's structured result. Only a complete tag with a recognized error name
is classified as an expected exception. Filesystem/engine failures remain
outside that catch. An engine timeout is classified as a resource limit, not
as proof of correctness. This tag is a test protocol, not an attestation of
exception provenance for arbitrary user-supplied JS: a deliberately forged
full tag is indistinguishable. Unknown outcomes remain unknown. This does
not prove blob replication or complete storage crash recovery.

Bounds: at most 512 UTF-8 source bytes, two executions, 64 MiB engine heap,
30-second engine timeout, one write per generated statement, three generated
statements, and at most one short partition followed by healing. Generated
strings are at most 12 input bytes. The general runner has a simulated-time
recovery deadline of 6 seconds. Generated programs cannot call console or form loops in the parser-mutation
branch; an arbitrary hand-written scenario is not output-bounded by this runner.
Do not use untrusted raw sources as fixtures without reviewing output behavior.

Run: `cargo +nightly fuzz run --manifest-path server/engine-cluster-tests/fuzz/Cargo.toml fuzz_js_unmodeled -- -max_len=64 -max_total_time=300 -timeout=120`.
On failure, `server/engine-cluster-tests/fuzz/target/scenario-artifacts/fuzz-js-unmodeled/scenario.json` records the source and seed. Re-run the scenario using
`replay_file(path)` or the fixture runner; preserve the libFuzzer crash input
and artifact before the next iteration overwrites that directory. LibFuzzer
can minimize its own input; promote a minimized serialized scenario into a
checked-in fixture. The runner currently does not enforce a console-output
quota or wall-clock watchdog outside the engine; do not claim those bounds.
