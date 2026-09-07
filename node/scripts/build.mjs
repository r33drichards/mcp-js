import { cpSync, readdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { esmImports } from './esm-imports.mjs';
import { checkElf } from './packaging.mjs';

process.chdir(fileURLToPath(new URL('..', import.meta.url)));
checkElf(readFileSync('generated/libserver.so'));
rmSync('dist', { recursive: true, force: true });
execFileSync(process.execPath, ['node_modules/typescript/bin/tsc', '-p', 'tsconfig.build.json'], { stdio: 'inherit' });
// The pinned generator emits extensionless imports; Node ESM needs .js even in declarations.
for (const name of readdirSync('dist').filter(n => n.endsWith('.js') || n.endsWith('.d.ts'))) {
  const text = readFileSync(`dist/${name}`, 'utf8');
  writeFileSync(`dist/${name}`, esmImports(text, name));
}
cpSync('generated/libserver.so', 'dist/libserver.so');
// Remove loader search paths from the distributable copy.
execFileSync('patchelf', ['--remove-rpath', 'dist/libserver.so'], { stdio: 'inherit' });
execFileSync('strip', ['--strip-unneeded', 'dist/libserver.so'], { stdio: 'inherit' });
cpSync('../LICENSE', 'LICENSE');
