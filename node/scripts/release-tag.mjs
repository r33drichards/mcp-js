import { readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

const identifier = '(?:0|[1-9]\\d*|\\d*[A-Za-z-][0-9A-Za-z-]*)';
const versionPattern = `(?:0|[1-9]\\d*)\\.(?:0|[1-9]\\d*)\\.(?:0|[1-9]\\d*)(?:-${identifier}(?:\\.${identifier})*)?(?:\\+[0-9A-Za-z-]+(?:\\.[0-9A-Za-z-]+)*)?`;
const pattern = new RegExp(`^v(${versionPattern})$`);

export function parseReleaseTag(tag) {
  const match = pattern.exec(tag);
  if (!match) throw new Error(`Invalid repository release tag: ${tag}`);
  const version = match[1];
  return { version, npmTag: version.includes('-') ? 'next' : 'latest' };
}

export function stagePackageVersion(directory, version) {
  parseReleaseTag(`v${version}`);
  for (const filename of ['package.json', 'package-lock.json']) {
    const path = join(directory, filename);
    const document = JSON.parse(readFileSync(path, 'utf8'));
    document.version = version;
    if (filename === 'package-lock.json') {
      if (!document.packages?.['']) throw new Error(`Missing root package entry in ${path}`);
      document.packages[''].version = version;
    }
    writeFileSync(path, `${JSON.stringify(document, null, 2)}\n`);
  }
}

const invoked = process.argv[1] && import.meta.url === new URL(`file://${process.argv[1]}`).href;
if (invoked) {
  if (process.argv[2] === '--stage') {
    stagePackageVersion(process.argv[3] ?? '', process.argv[4] ?? '');
  } else {
    process.stdout.write(`${JSON.stringify(parseReleaseTag(process.argv[2] ?? ''))}\n`);
  }
}
