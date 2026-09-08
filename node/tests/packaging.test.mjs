import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { execFileSync } from 'node:child_process';
import { join } from 'node:path';
import { checkDependencies, checkElf, checkTarball, packResult } from '../scripts/packaging.mjs';

test('both npm packages identify the trusted publishing repository', () => {
  for (const path of ['../package.json', '../../clients/typescript/package.json']) {
    const manifest = JSON.parse(readFileSync(new URL(path, import.meta.url), 'utf8'));
    assert.equal(manifest.repository?.type, 'git');
    assert.equal(manifest.repository?.url, 'https://github.com/r33drichards/mcp-js');
  }
});

test('repository release tags route stable and prerelease npm dist-tags', async () => {
  const { parseReleaseTag } = await import('../scripts/release-tag.mjs');
  assert.deepEqual(parseReleaseTag('v1.2.3'), { version: '1.2.3', npmTag: 'latest' });
  assert.deepEqual(parseReleaseTag('v2.0.0-rc.1'), { version: '2.0.0-rc.1', npmTag: 'next' });
  assert.deepEqual(parseReleaseTag('v0.19.0-rc2'), { version: '0.19.0-rc2', npmTag: 'next' });
  for (const tag of ['node-v1.2.3', 'v01.2.3', 'v1.2', 'v1.2.3-', 'release-v1.2.3']) {
    assert.throws(() => parseReleaseTag(tag));
  }
});

test('release staging updates package and lockfile root versions together', async () => {
  const { stagePackageVersion } = await import('../scripts/release-tag.mjs');
  const directory = mkdtempSync(join(tmpdir(), 'npm-version-'));
  try {
    writeFileSync(join(directory, 'package.json'), '{"name":"example","version":"0.1.0"}\n');
    writeFileSync(join(directory, 'package-lock.json'), '{"name":"example","version":"0.1.0","packages":{"":{"name":"example","version":"0.1.0"}}}\n');
    stagePackageVersion(directory, '1.2.3-rc.1');
    assert.equal(JSON.parse(readFileSync(join(directory, 'package.json'))).version, '1.2.3-rc.1');
    const lock = JSON.parse(readFileSync(join(directory, 'package-lock.json')));
    assert.equal(lock.version, '1.2.3-rc.1');
    assert.equal(lock.packages[''].version, '1.2.3-rc.1');
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

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
  checkDependencies(dynamic, ldd, Buffer.from('portable binary'));
  for (const bad of ['(RUNPATH) [/nix/store/lib]', '(RPATH) [/tmp/lib]', '(NEEDED) Shared library: [/tmp/lib.so]']) {
    assert.throws(() => checkDependencies(bad, ldd));
  }
  for (const bad of ['libfoo => not found', 'libc.so.6 => /nix/store/abc/libc.so.6', 'libc.so.6 => /home/runner/work/libc.so.6', 'statically linked', '']) {
    assert.throws(() => checkDependencies(dynamic, bad));
  }
});

test('tarball includes native library, ESM, types and license only', () => {
  const paths = ['package.json', 'README.md', 'LICENSE', 'dist/index.js', 'dist/index.d.ts', 'dist/libserver.so'];
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

test('release artifact staging follows the Nix result symlink', () => {
  const workflow = readFileSync(new URL('../../.github/workflows/npm-publish.yml', import.meta.url), 'utf8');
  const match = workflow.match(/- name: Stage exact tested tarball\n[\s\S]*?        run: \|\n((?:          .*\n)+)/);
  assert.ok(match, 'Missing release artifact staging script');
  const script = match[1].replace(/^          /gm, '');
  const directory = mkdtempSync(join(tmpdir(), 'npm-release-staging-'));
  try {
    mkdirSync(join(directory, 'nix-output'));
    writeFileSync(join(directory, 'nix-output', 'package.tgz'), 'validated tarball');
    symlinkSync('nix-output', join(directory, 'result'));
    execFileSync('bash', ['-e', '-c', script], { cwd: directory });
    assert.equal(readFileSync(join(directory, 'release-artifacts', 'package.tgz'), 'utf8'), 'validated tarball');
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});
