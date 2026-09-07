import assert from "node:assert/strict";
import { mkdtempSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { Engine } from "../generated/index";

test("native filesystem constructor enforces policy across calls", async () => {
  const dir = mkdtempSync(join(tmpdir(), "mcp-native-fs-"));
  const policy = join(dir, "policy.rego");
  const target = join(dir, "data.txt");
  writeFileSync(policy, `package mcp.filesystem\ndefault allow = false\nallow if { input.path == ${JSON.stringify(target)} }\n`);
  const config = JSON.stringify({ policies: [{ url: `file://${policy}` }] });
  const engine = Engine.createWithFilesystem(64n, 5n, config);
  const run = async (code: string): Promise<{ output?: string; error?: string }> => JSON.parse(
    await engine.callToolAsync("run_js", JSON.stringify({ code }), undefined, undefined),
  );
  try {
    assert.equal((await run(`await fs.writeFile(${JSON.stringify(target)}, "hello")`)).error, undefined);
    assert.equal((await run(`console.log(await fs.readFile(${JSON.stringify(target)}, "utf8"))`)).output?.trim(), "hello");
    assert.match((await run(`await fs.readFile(${JSON.stringify(policy)}, "utf8")`)).error ?? "", /denied by policy/);
    assert.throws(() => Engine.createWithFilesystem(64n, 5n, '{"policies":[]}'));
    assert.throws(() => Engine.createWithFilesystem(64n, 5n, 'invalid'));
  } finally {
    engine.close();
    engine.uniffiDestroy();
    rmSync(dir, { recursive: true, force: true });
  }
});
