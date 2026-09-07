import assert from 'node:assert/strict';

export function packResult(json) {
  const result = Array.isArray(json) ? json[0] : json['@wholelottahoopla/mcp-js-node'];
  assert.ok(result?.filename && Array.isArray(result.files), 'Unrecognized npm pack --json output');
  return result;
}

export function checkElf(bytes) {
  assert.ok(bytes.length >= 64, 'Native library is missing or truncated');
  assert.equal(bytes.subarray(0, 4).toString('hex'), '7f454c46', 'Native library must be ELF');
  assert.equal(bytes[4], 2, 'Native library must be 64-bit');
  assert.equal(bytes[5], 1, 'Native library must be little-endian');
  assert.equal(bytes.readUInt16LE(16), 3, 'Native library must be a shared object');
  assert.equal(bytes.readUInt16LE(18), 62, 'Native library must be x86-64');
}

export function checkDependencies(dynamic, dependencies, bytes) {
  assert.doesNotMatch(dynamic, /\((?:RPATH|RUNPATH)\)/, 'Remove build-host RPATH/RUNPATH before packaging');
  assert.doesNotMatch(dynamic, /Shared library: \[[^\]]*\//, 'Absolute/path-based DT_NEEDED is not portable');
  assert.doesNotMatch(dependencies, /not found|\/nix\/store\/|not a dynamic executable|statically linked/i,
    'Native dependencies must resolve outside the Nix store');
  assert.match(dependencies, /libc\.so/, 'Expected Linux glibc dynamic dependency report');
  if (bytes) {
    assert.doesNotMatch(bytes.toString('latin1'), /\/nix\/store\/|\/home\/runner\/work\//,
      'Native package must not retain Nix-store or CI-workspace paths');
  }
}

export function checkTarball(files) {
  const paths = files.map(f => f.path);
  for (const required of ['package.json', 'LICENSE', 'README.md', 'dist/index.js', 'dist/index.d.ts', 'dist/libserver.so']) {
    assert.ok(paths.includes(required), `Tarball missing ${required}`);
  }
  assert.ok(paths.every(p => ['package.json', 'LICENSE', 'README.md'].includes(p) ||
    /^dist\/(?:[\w-]+\.(?:js|d\.ts)|libserver\.so)$/.test(p)), 'Unexpected tarball payload');
}
