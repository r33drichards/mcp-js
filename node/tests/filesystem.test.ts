import assert from "node:assert/strict";
import { existsSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { Engine, type EngineLike, FsEntryKind } from "../generated/index";

type RunResult = { output?: string; error?: string };

function runner(engine: EngineLike): (code: string) => Promise<RunResult> {
  return async (code) =>
    JSON.parse(await engine.callToolAsync("run_js", JSON.stringify({ code }), undefined, undefined));
}

/** Every string a native failure carries, whatever shape the binding gives the error. */
function failureText(error: unknown): string {
  return [String(error), JSON.stringify(error)].join(" ");
}

async function rejectsWith(promise: Promise<unknown>, pattern: RegExp): Promise<void> {
  await assert.rejects(promise, (error: unknown) => {
    assert.match(failureText(error), pattern);
    return true;
  });
}

function release(engine: EngineLike): void {
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

test("native fs_* methods share the guest hook chain and report typed failures", async () => {
  const dir = mkdtempSync(join(tmpdir(), "mcp-native-fs-typed-"));
  const hook = join(dir, "hooks.rego");
  const alias = join(dir, "alias.bin");
  const redirected = join(dir, "redirected.bin");
  const denied = join(dir, "denied.txt");
  writeFileSync(denied, "secret");
  writeFileSync(
    hook,
    [
      "package mcp.filesystem",
      `pre := {"input": object.union(input, {"path": ${JSON.stringify(redirected)}})} if { input.path == ${JSON.stringify(alias)} }`,
      `pre := {"allow": false, "reason": "denied path"} if { input.path == ${JSON.stringify(denied)} }`,
      `pre := {"allow": false, "reason": "outside sandbox"} if { not startswith(input.path, ${JSON.stringify(dir)}) }`,
      "",
    ].join("\n"),
  );
  const engine = Engine.createWithFilesystem(64n, 5n, JSON.stringify({ pre: [{ url: `file://${hook}` }] }));
  const run = runner(engine);
  const bytes = new Uint8Array([0, 255, 10, 128]);
  try {
    assert.equal(engine.hostFilesystemEnabled(), true);

    // Native writes are rewritten by the same pre hook the guest sees.
    await engine.fsWriteFile(alias, bytes.buffer);
    assert.equal(existsSync(alias), false);
    assert.deepEqual(new Uint8Array(readFileSync(redirected)), bytes);
    assert.deepEqual(new Uint8Array(await engine.fsReadFile(alias)), bytes);
    assert.equal(
      (await run(`console.log(JSON.stringify(Array.from(await fs.readFile(${JSON.stringify(alias)}))))`)).output?.trim(),
      JSON.stringify(Array.from(bytes)),
    );

    // Text, append, metadata, listing, rename, remove.
    const text = join(dir, "notes.txt");
    await engine.fsWriteFile(text, new TextEncoder().encode("héllo").buffer);
    await engine.fsAppendFile(text, new TextEncoder().encode(" wörld").buffer);
    assert.equal(await engine.fsReadTextFile(text), "héllo wörld");
    assert.deepEqual(
      new Uint8Array(await engine.fsReadFileRange(text, 1n, 4n)),
      new TextEncoder().encode("héllo wörld").slice(1, 5),
    );
    assert.equal((await engine.fsReadFileRange(text, 100n, 4n)).byteLength, 0);
    assert.equal(await engine.fsCanonicalPath(text), realpathSync(text));
    const stat = await engine.fsStat(text);
    assert.equal(stat.kind, FsEntryKind.File);
    assert.equal(stat.size, BigInt(Buffer.byteLength("héllo wörld")));
    assert.equal(typeof stat.modifiedMs, "number");
    assert.equal((await engine.fsLstat(dir)).kind, FsEntryKind.Directory);
    await engine.fsMakeDir(join(dir, "a", "b"), true);
    assert.deepEqual((await engine.fsReadDir(join(dir, "a"))), ["b"]);
    await engine.fsRename(text, join(dir, "a", "b", "moved.txt"));
    assert.equal(await engine.fsExists(text), false);
    assert.equal(await engine.fsExists(join(dir, "a", "b", "moved.txt")), true);
    await engine.fsRemove(join(dir, "a"), true);
    assert.equal(await engine.fsExists(join(dir, "a")), false);

    // Typed failures: not found, invalid UTF-8, and hook denial.
    await rejectsWith(engine.fsReadFile(join(dir, "missing.txt")), /ENOENT/);
    await rejectsWith(engine.fsReadTextFile(redirected), /invalid UTF-8/);
    await rejectsWith(engine.fsReadFile(denied), /denied by pre hook \(denied path\)/);
    await rejectsWith(engine.fsReadDir("/etc"), /outside sandbox/);
    await rejectsWith(engine.fsCanonicalPath(join(dir, "missing.txt")), /ENOENT/);
    assert.equal(readFileSync(denied, "utf8"), "secret");

    // Engines without a filesystem configuration reject native calls.
    const plain = Engine.createStateless(64n, 1n);
    try {
      assert.equal(plain.hostFilesystemEnabled(), false);
      await rejectsWith(plain.fsExists(dir), /not configured/);
    } finally {
      release(plain);
    }
  } finally {
    release(engine);
    rmSync(dir, { recursive: true, force: true });
  }
});
