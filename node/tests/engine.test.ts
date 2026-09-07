import assert from "node:assert/strict";
import childProcess from "node:child_process";
import { syncBuiltinESMExports } from "node:module";
import { test } from "node:test";

test("Node executes JavaScript through the native UniFFI engine", async () => {
  // Imports and execution must not fall back to a CLI, Python, or an HTTP server.
  const methods = ["spawn", "spawnSync", "exec", "execSync", "execFile", "execFileSync", "fork"] as const;
  const originals = methods.map((name) => childProcess[name]);
  for (const name of methods) {
    Object.assign(childProcess, { [name]: () => { throw new Error("Unexpected subprocess"); } });
  }
  syncBuiltinESMExports();

  try {
    const { Engine, RuntimeLifecycleState } = await import("../generated/index");
    const engine = Engine.createStateless(64n, 1n);
    assert.ok(Engine.instanceOf(engine), "constructor must return a native Engine");
    const run = (code: string): { output: string; error?: string } =>
      JSON.parse(engine.callTool("run_js", JSON.stringify({ code }), undefined, undefined));

    try {
      assert.equal(engine.lifecycleState(), RuntimeLifecycleState.Running);
      const result = run("console.log(6 * 7)");
      assert.equal(result.error, undefined);
      assert.equal(result.output.trim(), "42");
      assert.equal(run('console.log(await Promise.resolve("awaited"))').output.trim(), "awaited");
      assert.match(run('throw new Error("node-uniffi-probe")').error ?? "", /node-uniffi-probe/);
      assert.ok(run("while (true) {}").error, "nonterminating JavaScript must time out");
      assert.equal(run('console.log("recovered")').output.trim(), "recovered");
      assert.equal(engine.close().alreadyShutdown, false);
      assert.equal(engine.lifecycleState(), RuntimeLifecycleState.Shutdown);
      assert.equal(engine.close().alreadyShutdown, true);
      assert.throws(() => run("console.log(1)"));
    } finally {
      engine.close();
      engine.uniffiDestroy();
    }

    for (const [memory, timeout] of [[0n, 1n], [64n, 0n], [4097n, 1n], [64n, 301n]]) {
      assert.throws(() => Engine.createStateless(memory, timeout));
    }
    console.log("Node native UniFFI E2E passed: 42, await, error, timeout recovery, shutdown");
  } finally {
    methods.forEach((name, i) => Object.assign(childProcess, { [name]: originals[i] }));
    syncBuiltinESMExports();
  }
});
