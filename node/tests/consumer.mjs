import assert from 'node:assert/strict';
import childProcess from 'node:child_process';
import { syncBuiltinESMExports } from 'node:module';
// Imports and calls may not use a CLI/server/Python fallback.
for (const method of ['spawn', 'spawnSync', 'exec', 'execSync', 'execFile', 'execFileSync', 'fork']) {
  childProcess[method] = () => { throw new Error('Unexpected subprocess'); };
}
syncBuiltinESMExports();
const { Engine, createToolRuntime } = await import('@wholelottahoopla/mcp-js-node');
const engine = Engine.createStateless(64n, 1n);
try {
  const result = JSON.parse(engine.callTool('run_js', JSON.stringify({ code: 'console.log(await Promise.resolve(6 * 7))' }), undefined, undefined));
  assert.equal(result.error, undefined);
  assert.equal(result.output.trim(), '42');
  const definition = {
    name: 'echo',
    description: 'Echo a message',
    inputSchema: { type: 'object', properties: { message: { type: 'string' } } },
  };
  const bind = prefix => createToolRuntime(engine, {
    server: 'host',
    tools: [definition],
    async call(name, args) {
      assert.equal(name, 'echo');
      await Promise.resolve();
      return prefix + args.message;
    },
  });
  const first = bind('first: ');
  const second = bind('second: ');
  assert.deepEqual(first.listTools()[0].inputSchema, definition.inputSchema);
  assert.equal(first.listTools()[0].name, 'runjs__host__echo');
  assert.match(first.listTools()[0].description, /Use run_js/);
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
  const hanging = createToolRuntime(engine, {
    server: 'host',
    tools: [definition],
    call: () => new Promise(() => {}),
  });
  const timedOut = await hanging.runJs(code);
  assert.equal(timedOut.status, 'timed_out');
  const controller = new AbortController();
  const cancelled = createToolRuntime(engine, {
    server: 'host',
    tools: [definition],
    call: () => {
      controller.abort();
      return null;
    },
  });
  await assert.rejects(cancelled.runJs(code, controller.signal), { name: 'AbortError' });

} finally {
  engine.close();
  engine.uniffiDestroy();
}
console.log('Installed native package executed JavaScript: 42');
