import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('.', import.meta.url));
const temp = mkdtempSync(join(tmpdir(), 'mcp-js-client-consumer-'));
const run = (command, args, cwd = temp) => execFileSync(command, args, {
  cwd, encoding: 'utf8', timeout: 180000, env: { ...process.env, NODE_PATH: '', NODE_OPTIONS: '' },
});
try {
  const output = JSON.parse(run('npm', ['pack', '--json', '--pack-destination', temp], root));
  const result = Array.isArray(output) ? output[0] : output['@wholelottahoopla/mcp-js-client'];
  assert.ok(result?.filename && Array.isArray(result.files), 'Unrecognized npm pack --json output');
  const paths = result.files.map(file => file.path);
  for (const required of ['package.json', 'README.md', 'LICENSE', 'dist/index.js', 'dist/index.d.ts', 'dist/schema.d.ts']) {
    assert.ok(paths.includes(required), `Tarball missing ${required}`);
  }
  assert.ok(paths.every(path => ['package.json', 'README.md', 'LICENSE'].includes(path) || /^dist\/(?:index\.(?:js|d\.ts)|schema\.d\.ts)$/.test(path)), 'Unexpected tarball payload');
  writeFileSync(join(temp, 'package.json'), JSON.stringify({ private: true, type: 'module' }));
  run('npm', ['install', '--ignore-scripts', '--no-audit', '--no-fund', join(temp, result.filename)]);
  writeFileSync(join(temp, 'consumer.mjs'), 'import { createMcpV8Client } from "@wholelottahoopla/mcp-js-client";\nconst client = createMcpV8Client("https://example.test");\nif (typeof client.runJs !== "function") throw new Error("missing client API");\nconsole.log("installed HTTP client imported");\n');
  writeFileSync(join(temp, 'consumer.ts'), 'import { createMcpV8Client, type ExecRequest } from "@wholelottahoopla/mcp-js-client";\nimport type { components } from "@wholelottahoopla/mcp-js-client/schema";\nconst request: ExecRequest = { code: "1 + 1" };\nconst accepted: components["schemas"]["ExecAccepted"] = { execution_id: "id" };\ncreateMcpV8Client("https://example.test").exec(request);\nvoid accepted;\n');
  run(process.execPath, [join(root, 'node_modules/typescript/bin/tsc'), '--noEmit', '--strict', '--skipLibCheck', '--target', 'ES2022', '--module', 'NodeNext', '--moduleResolution', 'NodeNext', 'consumer.ts']);
  console.log(run(process.execPath, ['consumer.mjs']));
} finally {
  rmSync(temp, { recursive: true, force: true });
}
