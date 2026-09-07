import { mkdtempSync, writeFileSync, cpSync, rmSync, renameSync, existsSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFileSync } from 'node:child_process';
import { packResult, checkDependencies, checkTarball } from './packaging.mjs';
const root = fileURLToPath(new URL('..', import.meta.url));
const temp = mkdtempSync(join(tmpdir(), 'mcp-node-consumer-'));
const run = (cmd, args, cwd = temp) => execFileSync(cmd, args, { cwd, encoding: 'utf8', timeout: 180000, env: { ...process.env, NODE_PATH: '', NODE_OPTIONS: '', LD_LIBRARY_PATH: '', LD_PRELOAD: '' } });
try {
  const result = packResult(JSON.parse(run('npm', ['pack', '--json', '--pack-destination', temp], root)));
  checkTarball(result.files);
  writeFileSync(join(temp, 'package.json'), JSON.stringify({ private: true, type: 'module' }));
  run('npm', ['install', '--ignore-scripts', '--no-audit', '--no-fund', join(temp, result.filename)]);
  const installedLibrary = join(temp, 'node_modules', '@wholelottahoopla', 'mcp-js-node', 'dist', 'libserver.so');
  checkDependencies(run('readelf', ['-d', installedLibrary]), run('ldd', [installedLibrary]));
  cpSync(join(root, 'tests/consumer.mjs'), join(temp, 'consumer.mjs'));
  writeFileSync(join(temp, 'consumer.ts'), 'import { Engine } from "@wholelottahoopla/mcp-js-node";\nconst engine: Engine = Engine.createStateless(64n, 1n);\nengine.close();\nengine.uniffiDestroy();\n');
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
  } finally {
    for (const [source, destination] of hidden.reverse()) renameSync(destination, source);
    // Keep backups if restoration fails rather than deleting checkout outputs.
    rmSync(backup, { recursive: true });
  }
} finally {
  rmSync(temp, { recursive: true, force: true });
}
