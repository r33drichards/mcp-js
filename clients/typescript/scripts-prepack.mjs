import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
process.chdir(fileURLToPath(new URL('.', import.meta.url)));
for (const file of ['dist/index.js', 'dist/index.d.ts', 'dist/schema.d.ts']) {
  assert.ok(existsSync(file), `Missing ${file}; run npm run build before packaging`);
}
assert.match(readFileSync('dist/index.d.ts', 'utf8'), /from "\.\/schema\.js"/, 'ESM must refer to schema with a NodeNext extension');
