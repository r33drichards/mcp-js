import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { BACKGROUND_CONTEXT, createEditTool, createReadTool, createRunJsTool, createWriteTool } from "@earendil-works/pi-agent-core";
import { McpJsExecutionEnv } from "@earendil-works/pi-agent-core/node";
import { Engine } from "../../generated/index";

const invocation = {
  invocationId: "invocation",
  operationId: "operation",
  turnId: "turn",
  getMemo: async () => undefined,
  setMemo: async () => {},
};
const noUpdate = () => {};

async function execute(tool: { execute: (...args: any[]) => Promise<any> }, input: unknown, env: McpJsExecutionEnv) {
  return tool.execute("call", input, noUpdate, { env }, invocation, BACKGROUND_CONTEXT);
}

function textOf(result: { content: Array<{ type: string; text?: string }> }): string {
  return result.content.filter((block) => block.type === "text").map((block) => block.text ?? "").join("\n");
}

test("pi file tools and run_js run against the native engine through one hook chain", async () => {
  const dir = mkdtempSync(join(tmpdir(), "pi-mcp-js-"));
  const policy = join(dir, "policy.rego");
  const work = join(dir, "work");
  writeFileSync(
    policy,
    `package mcp.filesystem\ndefault allow = false\nallow if { startswith(input.path, ${JSON.stringify(`${work}/`)}) }\nallow if { input.path == ${JSON.stringify(work)} }\n`,
  );
  const engine = Engine.createWithFilesystem(64n, 10n, JSON.stringify({ policies: [{ url: `file://${policy}` }] }));
  const env = new McpJsExecutionEnv(engine, work);
  try {
    assert.equal((await env.createDir(work, undefined, BACKGROUND_CONTEXT)).ok, true);

    // write -> read -> edit through the pi tools, with no shell and no guest JavaScript for files.
    const write = await execute(createWriteTool(), { path: "notes.txt", content: "alpha\nbeta\ngamma\n" }, env);
    assert.match(textOf(write), /notes\.txt/);
    assert.equal(readFileSync(join(work, "notes.txt"), "utf8"), "alpha\nbeta\ngamma\n");
    const read = await execute(createReadTool(), { path: "notes.txt", offset: 2, limit: 1 }, env);
    assert.match(textOf(read), /beta/);
    assert.doesNotMatch(textOf(read), /alpha|gamma/);
    await execute(createEditTool(), { path: "notes.txt", edits: [{ oldText: "beta", newText: "BETA" }] }, env);
    assert.equal(readFileSync(join(work, "notes.txt"), "utf8"), "alpha\nBETA\ngamma\n");

    // Binary bytes survive the typed boundary without JSON.
    const bytes = new Uint8Array([0, 255, 10, 128, 7]);
    assert.equal((await env.writeFile("blob.bin", bytes, BACKGROUND_CONTEXT)).ok, true);
    assert.deepEqual(new Uint8Array(readFileSync(join(work, "blob.bin"))), bytes);
    const back = await env.readBinaryFile("blob.bin", BACKGROUND_CONTEXT);
    assert.ok(back.ok);
    assert.deepEqual(back.value, bytes);

    // Line reads are bounded and stop early.
    const big = `first\n${"x".repeat(300_000)}\nlast\n`;
    writeFileSync(join(work, "big.txt"), big);
    const lines = await env.readTextLines("big.txt", { maxLines: 1 }, BACKGROUND_CONTEXT);
    assert.deepEqual(lines, { ok: true, value: ["first"] });

    // run_js sees the same files and the same policy.
    const js = await execute(
      createRunJsTool(),
      { code: `console.log(await fs.readFile(${JSON.stringify(join(work, "notes.txt"))}, "utf8"))` },
      env,
    );
    assert.equal(textOf(js).trim(), "alpha\nBETA\ngamma");
    await assert.rejects(
      execute(createRunJsTool(), { code: `await fs.readFile(${JSON.stringify(policy)}, "utf8")` }, env),
      /denied by policy/,
    );

    // Policy denials reach the tools as permission failures, never a Node fallback.
    const denied = await env.readTextFile(policy, BACKGROUND_CONTEXT);
    assert.equal(denied.ok, false);
    if (!denied.ok) assert.equal(denied.error.code, "permission_denied");
    const info = await env.fileInfo("notes.txt", BACKGROUND_CONTEXT);
    assert.ok(info.ok);
    assert.equal(info.value.kind, "file");
    assert.equal(info.value.size, Buffer.byteLength("alpha\nBETA\ngamma\n"));
    const listing = await env.listDir(".", BACKGROUND_CONTEXT);
    assert.ok(listing.ok);
    assert.deepEqual(listing.value.map((entry) => entry.name).sort(), ["big.txt", "blob.bin", "notes.txt"]);
    const missing = await env.readTextFile("missing.txt", BACKGROUND_CONTEXT);
    assert.equal(!missing.ok && missing.error.code, "not_found");
  } finally {
    await env.cleanup(BACKGROUND_CONTEXT);
    rmSync(dir, { recursive: true, force: true });
  }
});
