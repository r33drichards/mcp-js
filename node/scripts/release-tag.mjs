const identifier = '(?:0|[1-9]\\d*|\\d*[A-Za-z-][0-9A-Za-z-]*)';
const versionPattern = `(?:0|[1-9]\\d*)\\.(?:0|[1-9]\\d*)\\.(?:0|[1-9]\\d*)(?:-${identifier}(?:\\.${identifier})*)?(?:\\+[0-9A-Za-z-]+(?:\\.[0-9A-Za-z-]+)*)?`;
const pattern = new RegExp(`^(node|client)-v(${versionPattern})$`);

export function parseReleaseTag(tag) {
  const match = pattern.exec(tag);
  if (!match) throw new Error(`Invalid npm release tag: ${tag}`);
  const [, packageId, version] = match;
  return {
    packageId,
    version,
    npmTag: version.includes('-') ? 'next' : 'latest',
    packageName: packageId === 'node'
      ? '@wholelottahoopla/mcp-js-node'
      : '@wholelottahoopla/mcp-js-client',
  };
}

if (process.argv[1] && import.meta.url === new URL(`file://${process.argv[1]}`).href) {
  process.stdout.write(`${JSON.stringify(parseReleaseTag(process.argv[2] ?? ''))}\n`);
}
