import assert from "node:assert/strict";
import { existsSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import {
  BlobStoreBuilder,
  Engine,
  EngineConfigBuilder,
  ExecutionLimitsBuilder,
  FilesystemAccessBuilder,
  StoreBackend,
} from "../generated/index";

function release(engine: Engine): void {
  engine.close();
  if (Engine.instanceOf(engine)) engine.uniffiDestroy();
}

/** Submit through the execution API so the resulting heap hash is observable. */
async function runToCompletion(engine: Engine, code: string, heap: string | undefined) {
  const id = await engine.submitExecution({
    code,
    file: undefined,
    heap,
    fs: undefined,
    session: "pi-session",
    heapMemoryMaxMb: undefined,
    executionTimeoutSecs: undefined,
    tags: undefined,
    mcpHeaders: undefined,
  });
  for (;;) {
    const info = engine.getExecution(id);
    if (info.status === "completed") return info;
    if (info.status !== "pending" && info.status !== "running") throw new Error(`${info.status}: ${info.error}`);
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
}

test("builders assemble an engine with heap persistence and filesystem access", async () => {
  const dir = mkdtempSync(join(tmpdir(), "mcp-engine-config-"));
  const policy = join(dir, "policy.rego");
  writeFileSync(policy, "package mcp.filesystem\ndefault allow = false\n");
  const limits = new ExecutionLimitsBuilder().heapMemoryMaxMb(64n).executionTimeoutSecs(5n).build();
  assert.equal(limits.maxConcurrentExecutions, undefined);
  const config = new EngineConfigBuilder()
    .limits(limits)
    .dataDir(dir)
    .filesystem(
      new FilesystemAccessBuilder()
        .policiesJson(JSON.stringify({ policies: [{ url: `file://${policy}` }] }))
        .build(),
    )
    .heapStore(new BlobStoreBuilder().backend(StoreBackend.Directory).build())
    .build();

  let engine = Engine.create(config);
  let heap: string;
  try {
    assert.equal(engine.capabilities().heap, true);
    assert.equal(engine.hostFilesystemEnabled(), true);
    const first = await runToCompletion(engine, "globalThis.counter = 41;", undefined);
    assert.ok(first.heap, "a stateful execution reports its heap");
    heap = first.heap;
    assert.equal(existsSync(join(dir, "heaps")), true);
  } finally {
    release(engine);
  }

  // A second engine over the same data directory resumes the heap by hash, and
  // run_js accepts the hash directly.
  engine = Engine.create(config);
  try {
    const second = await runToCompletion(engine, "globalThis.counter += 1;", heap);
    assert.notEqual(second.heap, heap);
    const result = JSON.parse(
      await engine.callToolAsync(
        "run_js",
        JSON.stringify({ code: "console.log(globalThis.counter)", heap: second.heap }),
        undefined,
        undefined,
      ),
    );
    assert.equal(result.output?.trim(), "42");
  } finally {
    release(engine);
    rmSync(dir, { recursive: true, force: true });
  }

  assert.throws(() => new EngineConfigBuilder().build(), /EngineConfig is missing required field limits/);
  assert.throws(
    () => Engine.create(new EngineConfigBuilder().limits(limits).heapStore(new BlobStoreBuilder().backend(StoreBackend.S3).build()).build()),
    /bucket/,
  );
});
