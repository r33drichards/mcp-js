import assert from "node:assert/strict";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { Engine } from "../generated/index";

type RunResult = { output?: string; error?: string };

function runner(engine: Engine): (code: string) => Promise<RunResult> {
  return async (code) =>
    JSON.parse(await engine.callToolAsync("run_js", JSON.stringify({ code }), undefined, undefined));
}

function release(engine: Engine): void {
  engine.close();
  if (Engine.instanceOf(engine)) engine.uniffiDestroy();
}

test("native filesystem constructor enforces policy across calls", async () => {
  const dir = mkdtempSync(join(tmpdir(), "mcp-native-fs-"));
  const policy = join(dir, "policy.rego");
  const target = join(dir, "data.txt");
  writeFileSync(policy, `package mcp.filesystem\ndefault allow = false\nallow if { input.path == ${JSON.stringify(target)} }\n`);
  const config = JSON.stringify({ policies: [{ url: `file://${policy}` }] });
  const engine = Engine.createWithFilesystem(64n, 5n, config);
  const run = runner(engine);
  try {
    assert.equal((await run(`await fs.writeFile(${JSON.stringify(target)}, "hello")`)).error, undefined);
    assert.equal((await run(`console.log(await fs.readFile(${JSON.stringify(target)}, "utf8"))`)).output?.trim(), "hello");
    assert.match((await run(`await fs.readFile(${JSON.stringify(policy)}, "utf8")`)).error ?? "", /denied by policy/);
    assert.throws(() => Engine.createWithFilesystem(64n, 5n, '{"policies":[]}'));
    assert.throws(() => Engine.createWithFilesystem(64n, 5n, "{}"));
    assert.throws(() => Engine.createWithFilesystem(64n, 5n, "invalid"));
  } finally {
    release(engine);
    rmSync(dir, { recursive: true, force: true });
  }
});

test("native filesystem constructor accepts hook-only configuration and applies rewrites", async () => {
  const dir = mkdtempSync(join(tmpdir(), "mcp-native-fs-hooks-"));
  const hook = join(dir, "hooks.rego");
  const alias = join(dir, "alias.txt");
  const redirected = join(dir, "redirected.txt");
  writeFileSync(
    hook,
    [
      "package mcp.filesystem",
      `pre := {"input": object.union(input, {"path": ${JSON.stringify(redirected)}})} if { input.path == ${JSON.stringify(alias)} }`,
      `pre := {"allow": false, "reason": "outside sandbox"} if { not startswith(input.path, ${JSON.stringify(dir)}) }`,
      "",
    ].join("\n"),
  );
  const engine = Engine.createWithFilesystem(64n, 5n, JSON.stringify({ pre: [{ url: `file://${hook}` }] }));
  const run = runner(engine);
  try {
    assert.equal((await run(`await fs.writeFile(${JSON.stringify(alias)}, "moved")`)).error, undefined);
    assert.equal(existsSync(alias), false);
    assert.equal(readFileSync(redirected, "utf8"), "moved");
    assert.match((await run(`await fs.readFile("/etc/hostname", "utf8")`)).error ?? "", /outside sandbox/);
    assert.throws(() => Engine.createWithFilesystem(64n, 5n, JSON.stringify({ post: [{ url: `file://${hook}` }] })));
  } finally {
    release(engine);
    rmSync(dir, { recursive: true, force: true });
  }
});
