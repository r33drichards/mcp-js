import assert from "node:assert/strict";
import { existsSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import {
  BlobStoreBuilder,
  Engine,
  type EngineLike,
  EngineConfigBuilder,
  ExecutionLimitsBuilder,
  FilesystemAccessBuilder,
  StoreBackend,
} from "../generated/index";

function release(engine: EngineLike): void {
  engine.close();
  if (Engine.instanceOf(engine)) engine.uniffiDestroy();
}

/** Submit through the execution API so the resulting heap hash is observable. */
async function runToCompletion(engine: EngineLike, code: string, heap: string | undefined) {
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
    assert.equal(typeof result.execution_id, "string");
    const completed = await engine.awaitExecution(result.execution_id);
    assert.equal(completed.status, "completed", completed.error);
    const output = engine.getExecutionOutput(result.execution_id, undefined, undefined, undefined, undefined);
    assert.equal(output.data.trim(), "42");
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

test("session file views share the snapshot with guest runs", async () => {
  const dir = mkdtempSync(join(tmpdir(), "mcp-session-view-"));
  const policy = join(dir, "policy.rego");
  writeFileSync(policy, 'package mcp.filesystem\ndefault allow = false\nallow if { startswith(input.path, "/work") }\n');
  const config = new EngineConfigBuilder()
    .limits(new ExecutionLimitsBuilder().heapMemoryMaxMb(64n).executionTimeoutSecs(5n).build())
    .dataDir(dir)
    .filesystem(
      new FilesystemAccessBuilder()
        .policiesJson(JSON.stringify({ policies: [{ url: `file://${policy}` }] }))
        .build(),
    )
    .fsSnapshotStore(new BlobStoreBuilder().backend(StoreBackend.Directory).build())
    .heapStore(new BlobStoreBuilder().backend(StoreBackend.Directory).build())
    .build();
  const engine = Engine.create(config);
  // A stateful engine answers run_js with an execution id: await it, then read
  // the console output, the way an embedding host does.
  const runInSession = async (code: string): Promise<{ output?: string; error?: string }> => {
    const answer = JSON.parse(await engine.callToolAsync("run_js", JSON.stringify({ code }), "s1", undefined));
    if (typeof answer.execution_id !== "string") return answer;
    const info = await engine.awaitExecution(answer.execution_id);
    const page = engine.getExecutionOutput(answer.execution_id, undefined, undefined, undefined, undefined);
    return { output: page.data, error: info.status === "completed" ? undefined : (info.error ?? info.status) };
  };
  try {
    assert.equal(engine.capabilities().filesystem, true);
    const view = engine.fsView("s1");
    assert.equal(view.session(), "s1");
    await view.writeFile("/work/a.txt", new TextEncoder().encode("hello").buffer);
    assert.equal(await view.readTextFile("/work/a.txt"), "hello");
    const guest = await runInSession("globalThis.seen = await fs.readFile('/work/a.txt', 'utf8'); console.log(seen)");
    assert.equal(guest.output?.trim(), "hello", JSON.stringify(guest));
    await runInSession("await fs.writeFile('/work/b.txt', 'from guest')");
    assert.deepEqual((await view.readDir("/work")).sort(), ["a.txt", "b.txt"]);
    assert.equal(await view.readTextFile("/work/b.txt"), "from guest");
    const heapCheck = await runInSession("console.log(globalThis.seen)");
    assert.equal(heapCheck.output?.trim(), "hello", "the heap survives native mutations in between");
    const host = engine.fsView(undefined);
    assert.equal(host.session(), undefined);
    assert.equal(await host.exists("/work/a.txt"), false);
    const snapshots = await engine.listSessionSnapshots("s1");
    assert.equal(snapshots.length, 4);
    assert.match(snapshots[0].code, /^\/\/ native fs\.writeFile/);
  } finally {
    release(engine);
    rmSync(dir, { recursive: true, force: true });
  }
});
