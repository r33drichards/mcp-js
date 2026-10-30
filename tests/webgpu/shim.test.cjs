// Fast state-machine tests with a fake native boundary; real GPU coverage is in server/tests/webgpu.rs.
const { test } = require('node:test');
const assert = require('node:assert/strict');
const vm = require('node:vm');
const fs = require('node:fs');
const source = fs.readFileSync('server/src/engine/web_compat/webgpu.js', 'utf8');
function runtime() {
  let valid = true, calls = 0;
  const c = vm.createContext({ navigator: {}, structuredClone, Deno: { core: { ops: {
    op_gpu_request: async () => 'token',
    op_gpu_check: () => { if (!valid) throw Error('stale handle'); },
    op_gpu_destroy: () => { valid = false; },
    op_gpu_dispatch: async (_, json) => {
      calls++;
      const job = JSON.parse(json);
      const bytes = Uint8Array.from(job.buffers[0]);
      const words = new Uint32Array(bytes.buffer);
      for (let i = 0; i < words.length; i++) words[i] *= 2;
      return JSON.stringify([Array.from(bytes)]);
    }
  } } } });
  vm.runInContext(source, c);
  return { run: code => vm.runInContext('(async () => {' + code + '})()', c), stale: () => { valid = false; }, calls: () => calls };
}
test('upload, command encoding, readback and detach', async () => {
  const r = runtime();
  await r.run(fs.readFileSync('tests/webgpu/compute.js', 'utf8'));
  assert.equal(r.calls(), 1);
});
test('bounds, unsupported descriptors, cross-device forgery and lifecycle', async () => {
  const r = runtime();
  await r.run(`
    const a = await navigator.gpu.requestAdapter(); const d = await a.requestDevice();
    const bad = f => { let failed = false; try { f(); } catch { failed = true; } if (!failed) throw Error('accepted invalid call'); };
    bad(() => d.createBuffer({size: 4194308,usage:128}));
    bad(() => d.createBuffer({size: 3,usage:128}));
    bad(() => d.createBuffer({size: 4,usage:129}));
    bad(() => d.createBuffer({size: 4,usage:128,mappedAtCreation:true}));
    bad(() => d.createShaderModule({code:'', compilationHints:[]}));
    bad(() => d.queue.writeBuffer({},0,new Uint32Array([1])));
    const b = d.createBuffer({size:4,usage:136});
    bad(() => d.queue.writeBuffer(b,0,new Uint32Array([1,2])));
    b.destroy(); bad(() => d.queue.writeBuffer(b,0,new Uint32Array([1])));
    d.createBuffer({size:4194300,usage:128});
    bad(() => d.createBuffer({size:4,usage:128}));
    d.destroy(); bad(() => d.createCommandEncoder());
  `);
  assert.equal(r.calls(), 0);
});
test('restored native handles rejected', async () => {
  const r = runtime();
  await r.run('globalThis.device = await (await navigator.gpu.requestAdapter()).requestDevice();');
  r.stale();
  await assert.rejects(r.run('device.createBuffer({size:4,usage:128});'), /stale/);
});
