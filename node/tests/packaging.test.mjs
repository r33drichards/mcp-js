import assert from 'node:assert/strict';
import { test } from 'node:test';
import { checkDependencies, checkElf, checkTarball, packResult } from '../scripts/packaging.mjs';

test('npm pack JSON supports npm <=11 arrays and npm 12 keyed objects', () => {
  const result = { filename: 'mcp-js-node-0.1.0.tgz', files: [] };
  assert.equal(packResult([result]), result);
  assert.equal(packResult({ '@wholelottahoopla/mcp-js-node': result }), result);
  for (const bad of [{}, [], { '@wholelottahoopla/mcp-js-node': {} }]) assert.throws(() => packResult(bad));
});

test('native gate rejects absent, text, wrong architecture and executable files', () => {
  // Synthetic headers test only the parser, never native execution or package validation.
  const header = Buffer.alloc(64);
  header.write('7f454c46', 0, 'hex');
  header[4] = 2; header[5] = 1;
  header.writeUInt16LE(3, 16); header.writeUInt16LE(62, 18);
  checkElf(header);
  for (const bad of [Buffer.alloc(0), Buffer.alloc(64)]) assert.throws(() => checkElf(bad));
  for (const [offset, value] of [[4, 1], [5, 2], [16, 2], [18, 183]]) {
    const bad = Buffer.from(header); bad[offset] = value;
    assert.throws(() => checkElf(bad));
  }
});

test('portability gate rejects build-host paths and unresolved libraries', () => {
  const dynamic = '(NEEDED) Shared library: [libc.so.6]';
  const ldd = 'libc.so.6 => /lib/x86_64-linux-gnu/libc.so.6';
  checkDependencies(dynamic, ldd);
  for (const bad of ['(RUNPATH) [/nix/store/lib]', '(RPATH) [/tmp/lib]', '(NEEDED) Shared library: [/tmp/lib.so]']) {
    assert.throws(() => checkDependencies(bad, ldd));
  }
  for (const bad of ['libfoo => not found', 'libc.so.6 => /nix/store/abc/libc.so.6', 'statically linked', '']) {
    assert.throws(() => checkDependencies(dynamic, bad));
  }
});

test('tarball includes native library, ESM, types and license only', () => {
  const paths = ['package.json', 'README.md', 'LICENSE', 'dist/index.js', 'dist/index.d.ts', 'dist/libmcp_v8_uniffi.so'];
  const files = paths.map(path => ({ path }));
  checkTarball(files);
  for (let i = 0; i < files.length; i++) assert.throws(() => checkTarball(files.filter((_, j) => j !== i)));
  for (const path of ['generated/index.ts', 'tests/consumer.mjs', 'dist/secret.txt']) {
    assert.throws(() => checkTarball([...files, { path }]));
  }
});

test('ESM conversion fixes imports and exports without changing data or package imports', async () => {
  const { esmImports } = await import('../scripts/esm-imports.mjs');
  const source = `import x from './engine';\nexport * from "./types";\nimport('@ubjs/node');\nimport('./engine');\nconst data = './unchanged';\nexport * from './ready.js';`;
  assert.equal(esmImports(source, 'index.js'), `import x from './engine.js';\nexport * from "./types.js";\nimport('@ubjs/node');\nimport('./engine.js');\nconst data = './unchanged';\nexport * from './ready.js';`);
  assert.equal(esmImports('export { Engine } from "./engine";', 'index.d.ts'), 'export { Engine } from "./engine.js";');
});
