// Deliberately small compute-only WebGPU subset; see site-docs/how-to/webgpu.md.
(() => {
  const { op_gpu_request: request, op_gpu_dispatch: dispatch,
          op_gpu_destroy: destroy, op_gpu_check: check } = Deno.core.ops;
  const U = Object.freeze({ MAP_READ: 1, COPY_SRC: 4, COPY_DST: 8, STORAGE: 128 });
  const buffers = new WeakMap(), shaders = new WeakMap(), pipelines = new WeakMap();
  const layouts = new WeakMap(), groups = new WeakMap(), commands = new WeakMap();
  function fail(message) { throw new Error('webgpu: ' + message); }
  function fields(d, allowed) {
    if (!d || typeof d !== 'object') throw new TypeError('descriptor required');
    for (const key of Object.keys(d)) if (key !== 'label' && !allowed.includes(key)) fail('unsupported option: ' + key);
  }
  function integer(n, max, name) {
    if (!Number.isSafeInteger(n) || n < 0 || n > max) fail('invalid ' + name);
    return n;
  }
  function owned(map, obj, device) {
    const v = map.get(obj);
    if (!v || v.device !== device) fail('invalid or cross-device object');
    return v;
  }
  function live(device) { if (device.dead) fail('device destroyed'); check(device.token); }
  function buffer(obj, device) {
    live(device);
    const b = owned(buffers, obj, device);
    if (b.dead) fail('buffer destroyed');
    return b;
  }
  async function requestAdapter(options = {}) {
    fields(options, []);
    const token = await request();
    if (!token) return null;
    let used = false;
    return Object.freeze({ async requestDevice(options = {}) {
      fields(options, []);
      check(token);
      if (used) fail('one device per adapter');
      used = true;
      return makeDevice(token);
    } });
  }
  function makeDevice(token) {
    const d = { token, dead: false, bytes: 0, objects: 0, submissions: 0, pending: false, tail: Promise.resolve() };
    const count = () => { live(d); if (++d.objects > 128) fail('object creation limit (128)'); };
    const api = {
      destroy() { if (!d.dead) { d.dead = true; destroy(token); } },
      createBuffer(desc) {
        fields(desc, ['size', 'usage', 'mappedAtCreation']); count();
        const size = integer(desc.size, 4 * 1024 * 1024, 'buffer size');
        const usage = integer(desc.usage, 141, 'buffer usage');
        if (!size || size % 4 || !usage || (usage & ~141)) fail('buffer size/alignment/usage');
        if ((usage & U.MAP_READ) && (usage & ~9)) fail('MAP_READ only supports COPY_DST');
        if (desc.mappedAtCreation) fail('mappedAtCreation is unsupported');
        if (d.bytes + size > 4 * 1024 * 1024) fail('cumulative buffer limit (4 MiB)');
        d.bytes += size;
        const b = { device: d, data: new Uint8Array(size), size, usage, dead: false, mapped: null, mapping: false, returned: false };
        const obj = Object.freeze({ size, usage,
          get mapState() { return b.mapped ? 'mapped' : b.mapping ? 'pending' : 'unmapped'; },
          destroy() { b.dead = true; b.mapped = null; b.data = null; },
          async mapAsync(mode, offset = 0, length = size) {
            buffer(obj, d);
            if (mode !== 1 || !(usage & U.MAP_READ) || offset !== 0 || length !== size) fail('only full-buffer MAP_READ is supported');
            if (b.mapped || b.mapping) fail('buffer already mapping');
            b.mapping = true;
            try { await d.tail; buffer(obj, d); b.mapped = b.data.slice().buffer; b.returned = false; }
            finally { b.mapping = false; }
          },
          getMappedRange(offset = 0, length = size) {
            buffer(obj, d);
            if (!b.mapped || b.returned || offset !== 0 || length !== size) fail('only one full mapped range is supported');
            b.returned = true; return b.mapped;
          },
          unmap() {
            buffer(obj, d);
            if (b.mapping) fail('map is pending');
            // Mapped ranges are detached, matching WebGPU's lifetime contract.
            if (b.mapped) structuredClone(b.mapped, { transfer: [b.mapped] });
            b.mapped = null;
          }
        });
        buffers.set(obj, b); return obj;
      },
      createShaderModule(desc) {
        fields(desc, ['code']); count();
        if (typeof desc.code !== 'string' || desc.code.length > 65536) fail('shader source limit');
        const obj = Object.freeze({}); shaders.set(obj, { device: d, code: desc.code }); return obj;
      },
      createComputePipeline(desc) {
        fields(desc, ['layout', 'compute']); fields(desc.compute, ['module', 'entryPoint']); count();
        if (desc.layout !== 'auto') fail('only auto pipeline layout is supported');
        const shader = owned(shaders, desc.compute.module, d);
        const entry = desc.compute.entryPoint ?? 'main';
        if (typeof entry !== 'string' || entry.length > 256) fail('invalid entry point');
        const p = { device: d, code: shader.code, entry };
        const layout = Object.freeze({}); layouts.set(layout, p);
        const obj = Object.freeze({ getBindGroupLayout(index) { live(d); if (index !== 0) fail('only group 0 supported'); return layout; } });
        pipelines.set(obj, p); return obj;
      },
      createBindGroup(desc) {
        fields(desc, ['layout', 'entries']); count();
        const pipeline = owned(layouts, desc.layout, d);
        if (!Array.isArray(desc.entries) || !desc.entries.length || desc.entries.length > 8) fail('1–8 bindings required');
        const entries = desc.entries.map((entry, i) => {
          fields(entry, ['binding', 'resource']); fields(entry.resource, ['buffer']);
          if (entry.binding !== i) fail('bindings must be consecutive from 0');
          const b = buffer(entry.resource.buffer, d);
          if (!(b.usage & U.STORAGE)) fail('STORAGE usage required');
          return entry.resource.buffer;
        });
        if (new Set(entries).size !== entries.length) fail('aliased bindings unsupported');
        const obj = Object.freeze({}); groups.set(obj, { device: d, pipeline, entries }); return obj;
      },
      createCommandEncoder(desc = {}) {
        fields(desc, []); count();
        let begun = false, ended = false, finished = false, pipeline, group, workgroups;
        const copies = [];
        const editable = () => { live(d); if (finished) fail('encoder finished'); };
        return Object.freeze({
          beginComputePass(desc = {}) {
            fields(desc, []); editable(); if (begun) fail('one compute pass supported'); begun = true;
            const passLive = () => { editable(); if (ended) fail('pass ended'); };
            return Object.freeze({
              setPipeline(p) { passLive(); if (workgroups) fail('dispatch already encoded'); pipeline = owned(pipelines, p, d); },
              setBindGroup(index, g, offsets = []) { passLive(); if (workgroups) fail('dispatch already encoded'); if (index !== 0 || offsets.length) fail('only group 0 without dynamic offsets'); group = owned(groups, g, d); },
              dispatchWorkgroups(x, y = 1, z = 1) {
                passLive(); if (workgroups) fail('one dispatch supported');
                workgroups = [x, y, z].map(v => integer(v, 65536, 'workgroup count'));
                if (workgroups.some(v => !v) || x * y * z > 65536) fail('workgroup limit');
              },
              end() { passLive(); ended = true; }
            });
          },
          copyBufferToBuffer(src, srcOffset, dst, dstOffset, size) {
            editable(); if (!ended || copies.length >= 8) fail('copies must follow the compute pass; maximum 8');
            const a = buffer(src, d), b = buffer(dst, d);
            if (src === dst || srcOffset !== 0 || dstOffset !== 0 || size !== a.size || size !== b.size) fail('only full-buffer copies supported');
            if (!(a.usage & U.COPY_SRC) || !(b.usage & U.COPY_DST)) fail('copy usage flags required');
            copies.push([src, dst]);
          },
          finish(desc = {}) {
            fields(desc, []); editable();
            if (!ended || !workgroups || !pipeline || !group || group.pipeline !== pipeline) fail('incomplete or incompatible compute pass');
            finished = true;
            const obj = Object.freeze({}); commands.set(obj, { device: d, pipeline, group, workgroups, copies, used: false }); return obj;
          }
        });
      },
      queue: Object.freeze({
        writeBuffer(obj, offset, data, dataOffset = 0, size) {
          const b = buffer(obj, d);
          if (d.pending || b.mapped || b.mapping || !(b.usage & U.COPY_DST)) fail('buffer unavailable for upload');
          const view = ArrayBuffer.isView(data);
          if (!view && !(data instanceof ArrayBuffer)) throw new TypeError('BufferSource required');
          const unit = view ? (data.BYTES_PER_ELEMENT || 1) : 1;
          const source = new Uint8Array(view ? data.buffer : data, view ? data.byteOffset : 0, data.byteLength);
          integer(offset, b.size, 'offset'); integer(dataOffset, source.byteLength / unit, 'dataOffset');
          const bytes = size === undefined ? source.byteLength - dataOffset * unit : integer(size, source.byteLength / unit, 'size') * unit;
          if (offset % 4 || bytes % 4 || offset + bytes > b.size || dataOffset * unit + bytes > source.byteLength) fail('upload range/alignment');
          b.data.set(source.subarray(dataOffset * unit, dataOffset * unit + bytes), offset);
        },
        submit(list) {
          live(d);
          if (d.pending || !Array.isArray(list) || list.length !== 1) fail('one command buffer and one submission in flight supported');
          if (++d.submissions > 32) fail('submission limit (32)');
          const c = owned(commands, list[0], d); if (c.used) fail('command buffer already submitted');
          const targets = c.group.entries.map(obj => buffer(obj, d));
          const copies = c.copies.map(([a,b]) => [buffer(a,d),buffer(b,d)]);
          for (const b of [...targets, ...copies.flat()]) if (b.mapped || b.mapping) fail('mapped buffer cannot be submitted');
          c.used = true;
          const json = JSON.stringify({ code: c.pipeline.code, entry: c.pipeline.entry, workgroups: c.workgroups, buffers: targets.map(b => Array.from(b.data)) });
          d.pending = true;
          d.tail = dispatch(token, json).then(raw => {
            const result = JSON.parse(raw);
            for (let i = 0; i < targets.length; i++) if (!targets[i].dead) targets[i].data.set(result[i]);
            for (const [a,b] of copies) { if (a.dead || b.dead) fail('buffer destroyed during submission'); b.data.set(a.data); }
          }).finally(() => { d.pending = false; });
          // Retain the rejection for mapAsync/onSubmittedWorkDone, without an
          // unhandled rejection before callers attach their readback promise.
          d.tail.catch(() => {});
        },
        async onSubmittedWorkDone() { live(d); await d.tail; }
      })
    };
    return Object.freeze(api);
  }
  Object.defineProperty(navigator, 'gpu', { value: Object.freeze({ requestAdapter }), configurable: true });
  globalThis.GPUBufferUsage = U;
  globalThis.GPUMapMode = Object.freeze({ READ: 1 });
})();
