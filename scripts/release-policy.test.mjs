import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { chmodSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';
import { classifyRelease, dockerReleasePolicy } from './release-policy.mjs';

const root = fileURLToPath(new URL('../', import.meta.url));
const workflow = name => readFileSync(join(root, '.github/workflows', name), 'utf8');
const sha = 'a'.repeat(40);
const workflowSha = 'b'.repeat(40);

// Execute the actual checked-in workflow shell, with only network/mutating
// commands replaced. Tests never contact GitHub, Docker Hub, or a registry.
function stepScript(file, name) {
  const text = workflow(file);
  const start = text.indexOf(`      - name: ${name}\n`);
  assert.notEqual(start, -1, `missing step: ${name}`);
  const rest = text.slice(start);
  const run = rest.indexOf('        run: |\n');
  assert.notEqual(run, -1, `missing script: ${name}`);
  const lines = rest.slice(run + '        run: |\n'.length).split('\n');
  const body = [];
  for (const line of lines) {
    if (line && !line.startsWith('          ')) break;
    body.push(line.slice(10));
  }
  return body.join('\n');
}

function runStep(file, step, values = {}) {
  const dir = mkdtempSync(join(tmpdir(), 'mcp-release-policy-'));
  try {
    const calls = join(dir, 'calls.jsonl');
    const output = join(dir, 'output');
    writeFileSync(calls, '');
    writeFileSync(output, '');
    const stub = `#!${process.execPath}\n` + String.raw`
const fs = require('node:fs');
const path = require('node:path');
const command = path.basename(process.argv[1]);
const args = process.argv.slice(2);
fs.appendFileSync(process.env.MOCK_CALLS, JSON.stringify({ command, args }) + '\n');
if (command === 'curl') process.stdout.write(JSON.stringify({ tag_name: process.env.LATEST_TAG }));
if (command === 'git' && args[0] === 'rev-parse') process.stdout.write(process.env.TAG_SHA + '\n');
if (command === 'git' && args[0] === 'merge-base' && process.env.NOT_ON_MAIN === 'true') process.exit(1);
`;
    for (const command of ['gh', 'git', 'curl']) {
      writeFileSync(join(dir, command), stub);
      chmodSync(join(dir, command), 0o755);
    }
    const result = spawnSync('bash', ['-c', stepScript(file, step)], {
      cwd: root,
      encoding: 'utf8',
      env: {
        PATH: `${dir}:${process.env.PATH}`,
        HOME: dir,
        MOCK_CALLS: calls,
        GITHUB_OUTPUT: output,
        GITHUB_REPOSITORY: 'example/mcp-js',
        GITHUB_SHA: workflowSha,
        GH_TOKEN: '',
        TAG_SHA: sha,
        RUN_SHA: sha,
        LATEST_TAG: 'v0.22.0',
        INPUT_VERSION: '',
        RUN_TAG: '',
        NOTES: '',
        ...values,
      },
    });
    return {
      ...result,
      calls: readFileSync(calls, 'utf8').trim().split('\n').filter(Boolean).map(JSON.parse),
      output: Object.fromEntries(readFileSync(output, 'utf8').trim().split('\n').filter(Boolean).map(line => {
        const eq = line.indexOf('=');
        return [line.slice(0, eq), line.slice(eq + 1)];
      })),
    };
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

for (const [tag, prerelease] of [
  ['v0.23.0-rc.1', true], ['v1.2.3-beta', true], ['v1.2.3-0', true],
  ['v1.2.3-rc.2+build.5', true], ['v0.22.0', false],
  ['v1.2.3+build-rc.1', false],
]) {
  test(`classifies ${tag}`, () => {
    assert.equal(classifyRelease(tag).prerelease, prerelease);
    assert.equal(classifyRelease(tag).make_latest, prerelease ? 'false' : 'legacy');
  });
}

for (const tag of ['v01.2.3', 'v1.2.3-01', 'v1.2.3-', 'v1.2.3-rc..1', '1.2.3', 'v1.2.3\n', 'v1.2.3\ninjected=true']) {
  test(`rejects invalid tag ${JSON.stringify(tag)}`, () => assert.throws(() => classifyRelease(tag)));
}

test('rejects unknown event and non-boolean release policy', () => {
  assert.throws(() => dockerReleasePolicy('v1.2.3', 'unexpected', true));
  assert.throws(() => dockerReleasePolicy('v1.2.3', 'push', 'false'));
});

for (const notes of ['', 'Release notes with $shell and "quotes"\nand a second line']) {
  for (const prerelease of ['true', 'false']) {
    test(`release creation: prerelease=${prerelease}, supplied notes=${Boolean(notes)}`, () => {
      const result = runStep('cut-release.yml', 'Create release with notes', {
        VERSION: prerelease === 'true' ? 'v0.23.0-rc.1' : 'v0.23.0',
        PRERELEASE: prerelease,
        NOTES: notes,
      });
      assert.equal(result.status, 0, result.stderr);
      assert.equal(result.calls.length, 1);
      const { command, args } = result.calls[0];
      assert.equal(command, 'gh');
      assert.ok(args.includes(`--prerelease=${prerelease}`));
      assert.equal(args.includes('--latest=false'), prerelease === 'true');
      assert.ok(args.includes('--verify-tag'));
      assert.ok(!args.includes('--latest=true'));
      if (notes) assert.equal(args.at(-1), notes);
      else assert.equal(args.at(-1), '--generate-notes');
    });
  }
}

test('invalid release classification cannot create a release', () => {
  const result = runStep('cut-release.yml', 'Create release with notes', { VERSION: 'v0.23.0-rc.1', PRERELEASE: '' });
  assert.notEqual(result.status, 0);
  assert.equal(result.calls.length, 0);
});

for (const [name, values, expected] of [
  ['completed RC build', { EVENT_NAME: 'workflow_run', RUN_TAG: 'v0.23.0-rc.1' }, ['false', 'false', 'false', sha]],
  ['completed stable build', { EVENT_NAME: 'workflow_run', RUN_TAG: 'v0.23.0' }, ['true', 'true', 'true', sha]],
  ['explicit RC dispatch', { EVENT_NAME: 'workflow_dispatch', INPUT_VERSION: '0.23.0-rc.1' }, ['false', 'false', 'false', sha]],
  ['explicit stable dispatch', { EVENT_NAME: 'workflow_dispatch', INPUT_VERSION: 'v0.23.0' }, ['true', 'true', 'true', sha]],
  ['default manual refresh', { EVENT_NAME: 'workflow_dispatch' }, ['true', 'false', 'false', workflowSha]],
  ['main refresh', { EVENT_NAME: 'push' }, ['true', 'false', 'false', workflowSha]],
  ['PR validation', { EVENT_NAME: 'pull_request' }, ['false', 'false', 'false', workflowSha]],
]) {
  test(`Docker policy: ${name}`, () => {
    const result = runStep('docker-publish.yml', 'Resolve release to package', values);
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(['push_latest', 'stable_aliases', 'publish_mcp', 'source_sha'].map(key => result.output[key]), expected);
    if (values.EVENT_NAME === 'workflow_run' || values.INPUT_VERSION) {
      assert.equal(result.output.is_release, 'true');
      assert.ok(result.calls.some(call => call.command === 'git' && call.args[0] === 'merge-base'));
    } else {
      assert.equal(result.output.is_release, 'false');
      assert.ok(result.calls.some(call => call.command === 'curl' && call.args.at(-1).endsWith('/releases/latest')));
    }
  });
}

for (const [name, values] of [
  ['moved release tag', { EVENT_NAME: 'workflow_run', RUN_TAG: 'v0.23.0-rc.1', RUN_SHA: workflowSha }],
  ['missing source SHA', { EVENT_NAME: 'workflow_run', RUN_TAG: 'v0.23.0-rc.1', RUN_SHA: '' }],
  ['workflow run outside main', { EVENT_NAME: 'workflow_run', RUN_TAG: 'v0.23.0-rc.1', NOT_ON_MAIN: 'true' }],
  ['manual tag outside main', { EVENT_NAME: 'workflow_dispatch', INPUT_VERSION: 'v0.23.0-rc.1', NOT_ON_MAIN: 'true' }],
  ['RC mislabeled latest', { EVENT_NAME: 'workflow_dispatch', LATEST_TAG: 'v0.23.0-rc.1' }],
  ['invalid tag', { EVENT_NAME: 'workflow_dispatch', INPUT_VERSION: 'v0.23.0-rc..1' }],
]) {
  test(`Docker policy fails closed: ${name}`, () => {
    const result = runStep('docker-publish.yml', 'Resolve release to package', values);
    assert.notEqual(result.status, 0);
    assert.deepEqual(result.output, {});
  });
}

test('cut release accepts only an exact commit contained in main', () => {
  const good = runStep('cut-release.yml', 'Validate release policy and exact main commit', { VERSION: 'v0.23.0-rc.1', SHA: sha });
  assert.equal(good.status, 0, good.stderr);
  assert.equal(good.output.prerelease, 'true');
  for (const values of [{ SHA: 'main' }, { SHA: sha, NOT_ON_MAIN: 'true' }]) {
    const bad = runStep('cut-release.yml', 'Validate release policy and exact main commit', { VERSION: 'v0.23.0-rc.1', ...values });
    assert.notEqual(bad.status, 0);
    assert.ok(!bad.calls.some(call => call.command === 'gh'));
  }
});

test('binary release metadata preserves RC and stable channel policy', () => {
  for (const [TAG, prerelease, makeLatest] of [['v0.23.0-rc.1', 'true', 'false'], ['v0.23.0', 'false', 'legacy']]) {
    const result = runStep('release.yml', 'Classify GitHub release', { TAG });
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(result.output, { prerelease, make_latest: makeLatest });
  }
});

test('all publishing consumers are wired to the tested policy', () => {
  const docker = workflow('docker-publish.yml');
  assert.match(docker, /type=raw,value=latest,enable=\$\{\{ steps\.rel\.outputs\.push_latest \}\}/);
  assert.equal(docker.match(/enable=\$\{\{ steps\.rel\.outputs\.stable_aliases \}\}/g)?.length, 2);
  assert.match(docker, /type=semver,pattern=\{\{version\}\}.*enable=\$\{\{ steps\.rel\.outputs\.is_release \}\}/);
  assert.match(docker, /if: needs\.docker\.outputs\.publish_mcp == 'true'/);
  assert.match(docker, /ref: \$\{\{ steps\.rel\.outputs\.source_sha \}\}/);
  assert.match(docker, /ref: \$\{\{ needs\.docker\.outputs\.source_sha \}\}/);
  const release = workflow('release.yml');
  assert.match(release, /release:\n    if: startsWith\(github\.ref, 'refs\/tags\/v'\)/);
  assert.match(release, /prerelease: \$\{\{ steps\.policy\.outputs\.prerelease == 'true' \}\}/);
  assert.match(release, /make_latest: \$\{\{ steps\.policy\.outputs\.make_latest \}\}/);
});
