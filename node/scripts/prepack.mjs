import assert from 'node:assert/strict';
import { readFileSync, readdirSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { checkDependencies, checkElf } from './packaging.mjs';
process.chdir(fileURLToPath(new URL('..', import.meta.url)));
assert.equal(process.platform, 'linux');
assert.equal(process.arch, 'x64');
const library = 'dist/libserver.so';
checkElf(readFileSync(library));
assert.equal(readFileSync('LICENSE', 'utf8'), readFileSync('../LICENSE', 'utf8'), 'Package must include root license');
for (const file of ['dist/index.js', 'dist/index.d.ts']) assert.ok(readFileSync(file).length, `Empty ${file}`);
for (const file of readdirSync('dist').filter(f => f.endsWith('.js'))) {
  assert.doesNotMatch(readFileSync(`dist/${file}`, 'utf8'), /\boverride\s*:|\/nix\/store\//, 'Bindings must use colocated resolution');
}
const run = (command, args) => execFileSync(command, args, { encoding: 'utf8', timeout: 90000, env: { ...process.env, LD_LIBRARY_PATH: '', LD_PRELOAD: '', NODE_PATH: '', NODE_OPTIONS: '' } });
checkDependencies(run('readelf', ['-d', library]), run('ldd', [library]));
// Header checks alone cannot prove this is the engine. Execute the real API before packing.
console.error(run(process.execPath, ['tests/consumer.mjs']));
