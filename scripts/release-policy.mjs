import { readFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';
import { parseReleaseTag } from '../node/scripts/release-tag.mjs';

// Reuse the npm publisher's strict SemVer validation. A hyphen in build
// metadata is not a prerelease: v1.2.3+build-123 is a stable version.
export function classifyRelease(tag) {
  const { version } = parseReleaseTag(tag);
  if (tag !== `v${version}`) throw new Error('Release tag must match SemVer exactly');
  const prerelease = version.split('+')[0].includes('-');
  return { tag, version, prerelease, make_latest: prerelease ? 'false' : 'legacy' };
}

export function dockerReleasePolicy(tag, event, isRelease) {
  if (!['push', 'pull_request', 'workflow_dispatch', 'workflow_run'].includes(event)) {
    throw new Error(`Unsupported release event: ${event}`);
  }
  if (typeof isRelease !== 'boolean') throw new Error('isRelease must be a boolean');
  const release = classifyRelease(tag);
  if (!isRelease && release.prerelease) {
    throw new Error('A stable image refresh must not resolve to a prerelease');
  }
  const publish = event !== 'pull_request';
  return {
    ...release,
    is_release: isRelease,
    push_latest: publish && !release.prerelease,
    stable_aliases: publish && isRelease && !release.prerelease,
    publish_mcp: publish && isRelease && !release.prerelease,
  };
}

const dockerAssetNames = ['mcp-v8-linux.gz', 'mcp-v8-linux-arm64.gz'];

export function dockerAssetTag(release, stableOnly) {
  const { tag, prerelease } = classifyRelease(release?.tag_name);
  if (release.draft || (stableOnly && (release.prerelease || prerelease))) {
    throw new Error(`Docker cannot package ${tag} from this release channel`);
  }
  const missing = dockerAssetNames.filter(name =>
    !release.assets?.some(asset => asset.name === name && asset.state === 'uploaded' && asset.size > 0)
  );
  if (missing.length) throw new Error(`Release ${tag} is missing Docker assets: ${missing.join(', ')}`);
  return tag;
}

export function findDockerPrRelease(pages) {
  // gh --paginate --slurp returns one array per page in GitHub's order.
  for (const release of pages.flat()) {
    try {
      return dockerAssetTag(release, true);
    } catch {
      // Skip drafts, prereleases, and releases whose upload is not complete.
    }
  }
  throw new Error('No stable release has both Linux server binaries for Docker PR validation');
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const [mode, tag, event, release] = process.argv.slice(2);
  if (mode === 'docker-assets' && ['stable', 'explicit'].includes(tag)) {
    const selected = dockerAssetTag(JSON.parse(readFileSync(0, 'utf8')), tag === 'stable');
    process.stdout.write(`${selected}\n`);
    process.exit(0);
  }
  if (mode === 'docker-pr-fallback') {
    const selected = findDockerPrRelease(JSON.parse(readFileSync(0, 'utf8')));
    process.stdout.write(`${selected}\n`);
    process.exit(0);
  }
  let policy;
  if (mode === 'classify') policy = classifyRelease(tag);
  else if (mode === 'docker' && ['true', 'false'].includes(release)) {
    policy = dockerReleasePolicy(tag, event, release === 'true');
  } else throw new Error('Usage: release-policy.mjs classify TAG | docker TAG EVENT true|false | docker-assets stable|explicit | docker-pr-fallback');
  process.stdout.write(`${JSON.stringify(policy)}\n`);
}
