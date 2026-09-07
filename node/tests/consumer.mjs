import assert from 'node:assert/strict';
import childProcess from 'node:child_process';
import { syncBuiltinESMExports } from 'node:module';
// Imports and calls may not use a CLI/server/Python fallback.
for (const method of ['spawn', 'spawnSync', 'exec', 'execSync', 'execFile', 'execFileSync', 'fork']) {
  childProcess[method] = () => { throw new Error('Unexpected subprocess'); };
}
syncBuiltinESMExports();
const { Engine } = await import('@mcp-js/node');
const engine = Engine.createStateless(64n, 1n);
try {
  const result = JSON.parse(engine.callTool('run_js', JSON.stringify({ code: 'console.log(await Promise.resolve(6 * 7))' }), undefined, undefined));
  assert.equal(result.error, undefined);
  assert.equal(result.output.trim(), '42');
} finally {
  engine.close();
  engine.uniffiDestroy();
}
console.log('Installed native package executed JavaScript: 42');
