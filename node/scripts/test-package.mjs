import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync, readFileSync, cpSync, rmSync, renameSync, existsSync, mkdirSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFileSync } from 'node:child_process';
import { packResult, checkDependencies, checkTarball, checkElf } from './packaging.mjs';
const root = fileURLToPath(new URL('..', import.meta.url));
const temp = mkdtempSync(join(tmpdir(), 'mcp-node-consumer-'));
const run = (cmd, args, cwd = temp) => execFileSync(cmd, args, { cwd, encoding: 'utf8', timeout: 180000, env: { ...process.env, NODE_PATH: '', NODE_OPTIONS: '', LD_LIBRARY_PATH: '', LD_PRELOAD: '' } });
try {
  let tarball;
  if (process.argv[2]) {
    tarball = resolve(process.argv[2]);
    const paths = run('tar', ['-tzf', tarball]).trim().split('\n');
    checkTarball(paths.map(path => ({ path: path.replace(/^package\//, '') })));
  } else {
    const result = packResult(JSON.parse(run('npm', ['pack', '--json', '--pack-destination', temp], root)));
    checkTarball(result.files);
    tarball = join(temp, result.filename);
  }
  writeFileSync(join(temp, 'package.json'), JSON.stringify({ private: true, type: 'module' }));
  run('npm', ['install', '--ignore-scripts', '--no-audit', '--no-fund', tarball]);
  const installed = join(temp, 'node_modules', '@wholelottahoopla', 'mcp-js-node');
  const installedLibrary = join(installed, 'dist', 'libserver.so');
  const expectedLibc = JSON.parse(readFileSync(join(root, 'package.json'), 'utf8')).libc;
  const manifest = JSON.parse(readFileSync(join(installed, 'package.json'), 'utf8'));
  assert.deepEqual(manifest.libc, expectedLibc, 'Installed SDK targets the expected libc');
  checkElf(readFileSync(installedLibrary));
  if (readFileSync(join(installed, 'LICENSE'), 'utf8') !== readFileSync(join(root, '../LICENSE'), 'utf8')) throw new Error('Installed license differs from repository license');
  checkDependencies(run('readelf', ['-d', installedLibrary]), run('ldd', [installedLibrary]), expectedLibc[0]);
  cpSync(join(root, 'tests/consumer.mjs'), join(temp, 'consumer.mjs'));
  writeFileSync(join(temp, 'consumer.ts'), 'import { Engine } from "@wholelottahoopla/mcp-js-node";\nconst engine = Engine.createStateless(64n, 1n);\nengine.callTool("run_js", "{}", undefined, undefined);\nengine.dispose();\n');
  run(process.execPath, [join(root, 'node_modules/typescript/bin/tsc'), '--noEmit', '--strict', '--skipLibCheck', '--target', 'ES2022', '--module', 'NodeNext', '--moduleResolution', 'NodeNext', 'consumer.ts']);
  // Hide source outputs: loading must use only the installed tarball, not the checkout.
  const hidden = [];
  const backup = mkdtempSync(join(root, '.package-test-'));
  try {
    for (const directory of ['generated', 'dist']) {
      const source = join(root, directory);
      const destination = join(backup, directory);
      if (existsSync(source)) { renameSync(source, destination); hidden.push([source, destination]); }
    }
    console.log(run(process.execPath, ['consumer.mjs']));
    if (process.env.TEST_WITH_BUN === '1') {
      const bunConsumer = join(temp, 'bun-consumer');
      mkdirSync(bunConsumer);
      writeFileSync(join(bunConsumer, 'package.json'), JSON.stringify({
        private: true,
        type: 'module',
        dependencies: { [manifest.name]: `file:${tarball}` },
      }));
      cpSync(join(temp, 'consumer.mjs'), join(bunConsumer, 'consumer.mjs'));
      run('bun', ['install', '--ignore-scripts'], bunConsumer);
      console.log(run('bun', ['consumer.mjs'], bunConsumer));
    }
  } finally {
    for (const [source, destination] of hidden.reverse()) renameSync(destination, source);
    // Keep backups if restoration fails rather than deleting checkout outputs.
    rmSync(backup, { recursive: true });
  }
} finally {
  rmSync(temp, { recursive: true, force: true });
}
