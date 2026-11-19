import assert from 'node:assert/strict';
import childProcess from 'node:child_process';
import { syncBuiltinESMExports } from 'node:module';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
// Imports and calls may not use a CLI/server/Python fallback.
for (const method of ['spawn', 'spawnSync', 'exec', 'execSync', 'execFile', 'execFileSync', 'fork']) {
  childProcess[method] = () => { throw new Error('Unexpected subprocess'); };
}
syncBuiltinESMExports();
const { Engine, EngineConfigBuilder, ExecutionLimitsBuilder, ToolCallRequestBuilder, ToolRuntimeBuilder, createToolRuntime } = await import('@wholelottahoopla/mcp-js-node');
const engine = Engine.createStateless(64n, 1n);
try {
  const result = JSON.parse(engine.callTool('run_js', JSON.stringify({ code: 'console.log(await Promise.resolve(6 * 7))' }), undefined, undefined));
  assert.equal(result.error, undefined);
  assert.equal(result.output.trim(), '42');
  const request = new ToolCallRequestBuilder()
    .name('run_js')
    .argumentsJson(JSON.stringify({ code: 'console.log(7 * 6)' }))
    .build();
  const typed = JSON.parse(await engine.invokeTool(request));
  assert.deepEqual(typed, result);

  const definition = {
    description: 'Echo a message',
    inputSchema: { type: 'object', properties: { message: { type: 'string' } } },
  };
  const base = new ToolRuntimeBuilder().server('host');
  assert.throws(() => new ToolRuntimeBuilder().build(), /Host tool server/);
  const bind = prefix => createToolRuntime(engine, base.registerTool('echo', {
    ...definition,
    async run(args) {
      await Promise.resolve();
      return prefix + args.message;
    },
  }).build());
  const noStubs = base.registerTool('echo', { ...definition, run: args => args })
    .stubs(false).build();
  assert.equal(base.build().tools.size, 0);
  assert.deepEqual(createToolRuntime(engine, noStubs).listTools(), []);
  const registered = base.registerTool('echo', { ...definition, run: args => args });
  assert.throws(() => registered.registerTool('echo', { ...definition, run: args => args }), /unique/);
  const first = bind('first: ');
  const second = bind('second: ');
  assert.deepEqual(first.listTools()[0].inputSchema, definition.inputSchema);
  assert.equal(first.listTools()[0].name, 'runjs__host__echo');
  assert.match(first.listTools()[0].description, /invoke via run_js/);
  assert.match(first.stubInstructions('runjs__host__echo', { message: 'hello' }), /await mcp.callTool/);
  const code = `
    const tool = mcp.listTools("host")[0];
    const result = await mcp.callTool("host", tool.name, { message: "hello" });
    console.log(JSON.parse(result.content[0].text));
  `;
  const [a, b] = await Promise.all([first.runJs(code), second.runJs(code)]);
  assert.equal(a.status, 'completed', JSON.stringify(a));
  assert.equal(b.status, 'completed', JSON.stringify(b));
  assert.equal(a.output.trim(), 'first: hello');
  assert.equal(b.output.trim(), 'second: hello');
  assert.equal((await first.runJs(code)).output.trim(), 'first: hello');
  const unknown = await first.runJs('await mcp.callTool("host", "missing", {})');
  assert.equal(unknown.status, 'failed');
  assert.match(unknown.error, /Unknown host tool/);
  const hanging = createToolRuntime(engine, base.registerTool('echo', {
    ...definition,
    run: () => new Promise(() => {}),
  }).build());
  const timedOut = await hanging.runJs(code);
  assert.equal(timedOut.status, 'timed_out');
  const controller = new AbortController();
  const cancelled = createToolRuntime(engine, base.registerTool('echo', {
    ...definition,
    run() {
      controller.abort();
      return null;
    },
  }).build());
  await assert.rejects(cancelled.runJs(code, controller.signal), { name: 'AbortError' });

} finally {
  engine.close();
  engine.uniffiDestroy();
}
const hookDirectory = mkdtempSync(join(tmpdir(), 'mcp-host-hooks-'));
try {
  const hookFile = join(hookDirectory, 'hooks.js');
  writeFileSync(hookFile, `
    function pre(input) {
      return { input: { ...input, arguments: { ...input.arguments, rewritten: true } } };
    }
    function post(input, output) {
      return { output: { ...output, content: [{ type: "text", text: "post: " + output.content[0].text }] } };
    }
  `);
  const url = pathToFileURL(hookFile).href;
  const policyFile = join(hookDirectory, 'mcp-tools.rego');
  writeFileSync(policyFile, `package mcp.tools
default allow := false
allow if {
  input.server == "host"
  input.tool == "echo"
  input.arguments.rewritten == true
  input.arguments.message != "denied"
}`);
  const config = new EngineConfigBuilder()
    .limits(new ExecutionLimitsBuilder().heapMemoryMaxMb(64n).executionTimeoutSecs(5n).build())
    .mcpToolsPoliciesJson(JSON.stringify({
      pre: [{ url }],
      policies: [{ url: pathToFileURL(policyFile).href }],
      post: [{ url }],
    }))
    .build();
  const hookedEngine = Engine.create(config);
  try {
    let calls = 0;
    const toolConfig = new ToolRuntimeBuilder()
      .server('host')
      .stubPrefix('discover__')
      .registerTool('echo', {
        description: 'Echo a message',
        inputSchema: { type: 'object' },
        run(args) {
          calls++;
          assert.equal(args.rewritten, true);
          return args.message;
        },
      })
      .build();
    const runtime = createToolRuntime(hookedEngine, toolConfig);
    assert.equal(runtime.listTools()[0].name, 'discover__host__echo');
    const success = await runtime.runJs('console.log((await mcp.callTool("host", "echo", { message: "allowed" })).content[0].text)');
    assert.equal(success.status, 'completed', JSON.stringify(success));
    assert.equal(success.output.trim(), 'post: "allowed"');

    const denied = await runtime.runJs('await mcp.callTool("host", "echo", { message: "denied" })');
    assert.equal(denied.status, 'failed');
    assert.match(denied.error, /is not allowed/);
    assert.equal(calls, 1);
  } finally {
    hookedEngine.close();
    hookedEngine.uniffiDestroy();
  }
} finally {
  rmSync(hookDirectory, { recursive: true, force: true });
}
console.log('Installed native package executed JavaScript: 42');
