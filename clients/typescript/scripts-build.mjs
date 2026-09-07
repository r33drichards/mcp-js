import { cpSync, existsSync, rmSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

process.chdir(fileURLToPath(new URL('.', import.meta.url)));
rmSync('dist', { recursive: true, force: true });
execFileSync(process.execPath, ['node_modules/typescript/bin/tsc', '-p', 'tsconfig.build.json'], { stdio: 'inherit' });
if (!existsSync('dist/index.js') || !existsSync('dist/index.d.ts')) {
  throw new Error('TypeScript build did not emit ESM and declarations');
}
// TypeScript preserves .d.ts source files only when they are explicitly copied.
cpSync('src/schema.d.ts', 'dist/schema.d.ts');
