#!/usr/bin/env python3
"""Measure artifact reuse independently from uncached runtime test execution."""
import argparse
import datetime
import json
import os
from pathlib import Path
import re
import subprocess
import time

parser = argparse.ArgumentParser()
parser.add_argument('--implementation', choices=['crane', 'cargo-nix'], required=True)
parser.add_argument('--results', type=Path, required=True)
args = parser.parse_args()
repo = Path.cwd()
out = args.results.resolve()
out.mkdir(parents=True, exist_ok=True)
# Keep GC symlinks out of the uploaded evidence tree (test binaries are large).
roots = out.parent / (out.name + '-gc-roots')
roots.mkdir(parents=True, exist_ok=True)
expected = json.loads((repo / 'scripts/ci/expected-nix-comparison-tests.json').read_text())
report = {
    'implementation': args.implementation,
    'commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(),
    'application_baseline': expected['baseline_commit'],
    'started_at': datetime.datetime.now(datetime.timezone.utc).isoformat(),
    'run_attempt': os.environ.get('GITHUB_RUN_ATTEMPT'),
    'store_cache_restored': os.environ.get('STORE_CACHE_RESTORED', ''),
    'cpu_count': os.cpu_count(),
    'nix_max_jobs': 2,
    'nix_cores': 2,
    'phases': [],
    'status': 'running',
}
result_file = out / 'results.json'

def save():
    result_file.write_text(json.dumps(report, indent=2) + '\n')


def command(phase, argv, *, json_output=False, timeout=3600):
    print(f'::group::{phase}', flush=True)
    print('Command: ' + json.dumps([str(x) for x in argv]), flush=True)
    start = time.monotonic()
    stdout = out / f'{phase}.stdout.log'
    stderr = out / f'{phase}.stderr.log'
    record = {'name': phase, 'command': [str(x) for x in argv]}
    report['phases'].append(record)
    save()
    try:
        with stdout.open('wb') as output, stderr.open('wb') as error:
            proc = subprocess.run(argv, stdout=output, stderr=error, timeout=timeout)
        record['returncode'] = proc.returncode
    except subprocess.TimeoutExpired:
        record['returncode'] = 124
    record['seconds'] = round(time.monotonic() - start, 3)
    save()
    print(f"{phase}: {record['seconds']}s, exit {record['returncode']}", flush=True)
    if record['returncode']:
        for log in [stdout, stderr]:
            print('\n'.join(log.read_text(errors='replace').splitlines()[-80:]), flush=True)
        raise RuntimeError(f'{phase} failed; see uploaded phase logs')
    print('::endgroup::', flush=True)
    text = stdout.read_text()
    return json.loads(text) if json_output else text


def outputs(build_json):
    paths = []
    for item in build_json:
        for value in item['outputs'].values():
            path = value if isinstance(value, str) else value['path']
            paths.append(path)
    return sorted(set(paths))


def build(phase, nix, *attrs):
    return outputs(command(phase, nix + ['build', '--json', '--no-link',
        '--max-jobs', '2', '--cores', '2', *attrs], json_output=True))


def root_outputs(paths, phase, nix_bin):
    for index, path in enumerate(paths):
        command(f'{phase}-root-{index}', [str(nix_bin / 'nix-store'), '--add-root',
            str(roots / f'{phase}-{index}'), '--indirect', '--realise', path])


def select(paths, suffix):
    matches = [Path(p) for p in paths if (Path(p) / suffix).exists()]
    if len(matches) != 1:
        raise RuntimeError(f'Expected one output containing {suffix}, found {matches}')
    return matches[0]


def inventory(tests):
    found = json.loads((tests / 'share/benchmark-targets.json').read_text())
    def kind(value):
        return value[0] if isinstance(value, list) else value
    actual = {(t['name'], kind(t['kind'])) for t in found['targets']}
    wanted = {(t['name'], t['kind'][0]) for t in expected['targets']
        if t['test'] and t['kind'][0] in ['lib', 'bin', 'test']}
    if actual != wanted:
        raise RuntimeError(f'Target parity failed; missing={wanted-actual}, extra={actual-wanted}')
    actual_examples = {t['name'] for t in found['example_targets']}
    wanted_examples = {t['name'] for t in expected['targets'] if 'example' in t['kind']}
    if actual_examples != wanted_examples or found.get('doctests') is not True:
        raise RuntimeError('Example/doctest coverage does not match baseline')
    binary_sizes = {}
    for target in found['targets'] + found['example_targets']:
        path = Path(target['path'])
        if not path.is_absolute():
            path = tests / path
        if not path.is_file():
            raise RuntimeError(f'Missing compiled target: {target["name"]}: {path}')
        binary_sizes[target['path']] = path.stat().st_size
    report['inventory'] = {
        'runtime_targets': len(actual), 'examples': sorted(actual_examples), 'doctests': True,
        'runtime_binary_bytes': sum(binary_sizes[t['path']] for t in found['targets']),
        'example_binary_bytes': sum(binary_sizes[t['path']] for t in found['example_targets']),
    }
    (out / 'compiled-targets.json').write_text(json.dumps(found, indent=2))
    save()


def runtime_results(text):
    summaries = re.findall(r'test result: (?:ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored;', text)
    counts = {'passed': sum(int(x[0]) for x in summaries),
        'failed': sum(int(x[1]) for x in summaries),
        'ignored': sum(int(x[2]) for x in summaries), 'summaries': len(summaries)}
    report['tests'] = counts
    save()
    wanted = {'passed': expected['baseline_passed'], 'failed': 0,
        'ignored': expected['baseline_ignored'], 'summaries': 82}
    if counts != wanted:
        raise RuntimeError(f'Test parity failed: {counts}; expected {wanted}')


original_file = repo / 'server/src/lib.rs'
original_bytes = original_file.read_bytes()
try:
    # Same bootstrap output on both branches; plugin compilation is separate.
    boot = build('bootstrap-runtime-and-sources', ['nix'], '.#benchmark-nix', '.#benchmark-source-roots', '.#benchmark-config')
    runtime = select(boot, 'bin/nix')
    os.environ['PATH'] = str(runtime / 'bin') + os.pathsep + os.environ['PATH']
    config_candidates = []
    for output in boot:
        candidate = Path(output)
        if candidate.is_file():
            try:
                config = json.loads(candidate.read_text())
            except (ValueError, OSError, UnicodeDecodeError):
                continue
            if isinstance(config, dict) and 'implementation' in config:
                config_candidates.append(config)
    if len(config_candidates) != 1:
        raise RuntimeError('Expected one JSON benchmark-config output')
    report['configuration'] = config_candidates[0]
    (out / 'configuration.json').write_text(json.dumps(config_candidates[0], indent=2))
    root_outputs(boot, 'bootstrap', runtime / 'bin')
    if os.environ.get('GITHUB_PATH'):
        with open(os.environ['GITHUB_PATH'], 'a') as path_file:
            path_file.write(str(runtime / 'bin') + '\n')
    nix = [str(runtime / 'bin/nix')]
    report['nix_version'] = command('nix-version', nix + ['--version']).strip()
    report['runtime_outputs'] = boot
    report['nix_runtime'] = str(runtime)
    if args.implementation == 'cargo-nix':
        plugin_paths = build('bootstrap-plugin', nix, '.#benchmark-plugin')
        plugin = select(plugin_paths, 'lib/nix/plugins')
        root_outputs(plugin_paths, 'plugin', runtime / 'bin')
        command('prefetch-index', [str(plugin / 'bin/cargo-nix-prefetch'), '--manifest-path', str(repo / 'Cargo.toml')])
        command('verify-index', [str(plugin / 'bin/cargo-nix-prefetch'), '--manifest-path', str(repo / 'Cargo.toml'), '--check'])
        nix += ['--option', 'plugin-files', str(plugin / 'lib/nix/plugins')]
    original = build('artifact-build', nix, '.#benchmark-tests', '.#benchmark-runner')
    root_outputs(original, 'original', runtime / 'bin')
    tests = select(original, 'share/benchmark-targets.json')
    runner = select(original, 'bin/run-benchmark-tests')
    inventory(tests)
    for evidence in (tests / 'share').glob('*'):
        if evidence.is_file() and ('feature' in evidence.name or 'graph' in evidence.name):
            (out / evidence.name).write_bytes(evidence.read_bytes())
    warm = build('exact-warm-artifact-build', nix, '.#benchmark-tests', '.#benchmark-runner')
    if warm != original:
        raise RuntimeError('Identical build did not resolve to identical output paths')
    report['exact_warm_same_outputs'] = True
    report['original_outputs'] = original
    before = set(command('original-closure', [str(runtime / 'bin/nix-store'), '--query', '--requisites', str(tests)]).splitlines())
    # Always execute, even if the test-artifact derivation came from a cache.
    test_text = command('runtime-tests', [str(runner / 'bin/run-benchmark-tests')])
    test_text += (out / 'runtime-tests.stderr.log').read_text(errors='replace')
    runtime_results(test_text)
    # Test reuse when only application source changes, not dependencies/config.
    original_file.write_bytes(original_bytes + b'\n// CI build-cache comparison: source-only invalidation.\n')
    changed = build('source-only-rebuild', nix, '.#benchmark-tests', '.#benchmark-runner')
    root_outputs(changed, 'changed', runtime / 'bin')
    changed_tests = select(changed, 'share/benchmark-targets.json')
    if str(changed_tests) == str(tests):
        raise RuntimeError('Source change did not invalidate the compiled test output')
    after = set(command('changed-closure', [str(runtime / 'bin/nix-store'), '--query', '--requisites', str(changed_tests)]).splitlines())
    report['closure_reuse'] = {'unchanged_paths': len(before & after), 'new_paths': len(after-before), 'old_paths': len(before-after)}
    changed_warm = build('source-change-exact-warm', nix, '.#benchmark-tests', '.#benchmark-runner')
    if changed_warm != changed:
        raise RuntimeError('Changed-source warm build did not retain identical outputs')
    report['source_change_invalidated'] = True
    report['changed_outputs'] = changed
    report['status'] = 'success'
except Exception as error:
    report['status'] = 'failure'
    report['error'] = str(error)
    raise
finally:
    original_file.write_bytes(original_bytes)
    report['finished_at'] = datetime.datetime.now(datetime.timezone.utc).isoformat()
    save()
    print(result_file.read_text(), flush=True)
