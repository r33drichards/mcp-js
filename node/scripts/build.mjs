import { cpSync, readdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { esmImports } from './esm-imports.mjs';
import { checkElf } from './packaging.mjs';

process.chdir(fileURLToPath(new URL('..', import.meta.url)));
checkElf(readFileSync('generated/libmcp_v8_uniffi.so'));
rmSync('dist', { recursive: true, force: true });
execFileSync(process.execPath, ['node_modules/typescript/bin/tsc', '-p', 'tsconfig.build.json'], { stdio: 'inherit' });
// The pinned generator emits extensionless imports; Node ESM needs .js even in declarations.
for (const name of readdirSync('dist').filter(n => n.endsWith('.js') || n.endsWith('.d.ts'))) {
  const text = readFileSync(`dist/${name}`, 'utf8');
  writeFileSync(`dist/${name}`, esmImports(text, name));
}
cpSync('generated/libmcp_v8_uniffi.so', 'dist/libmcp_v8_uniffi.so');
cpSync('../LICENSE', 'LICENSE');
