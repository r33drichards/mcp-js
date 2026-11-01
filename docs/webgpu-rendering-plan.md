# WebGPU gallery evidence and rendering milestones

The published compute baseline is `de267bb5dbd24177f733020d1a0fe12476a779c7`
on `feat/webgpu-compute`. This work is local on `feat/webgpu-rendering-local`.
Nothing in this new scope has been pushed. The local implementation adds real
wgpu offscreen rendering and compute uniform buffers; it does not implement all
WebGPU operations or claim browser/demo compatibility.

## Pinned gallery sources

Source: [WebGPU Samples gallery](https://webgpu.github.io/webgpu-samples/), repository
commit [`e040ec1a20dbbe01afed30c812e825b2639cfe5b`](https://github.com/webgpu/webgpu-samples/tree/e040ec1a20dbbe01afed30c812e825b2639cfe5b).
The Boids sample is a plausible match for the remembered demo, not a confirmed
identification. Original fixtures, their SHA-256 hashes, source URLs, and the
upstream BSD-3-Clause license are in `tests/webgpu/gallery/upstream/` and
`sources.json`. Fixtures were verified byte-for-byte against the pinned checkout.

| Workload | Shader/API adaptations | Verified result |
| --- | --- | --- |
| [Boids compute](https://github.com/webgpu/webgpu-samples/blob/e040ec1a20dbbe01afed30c812e825b2639cfe5b/sample/computeBoids/updateSprites.wgsl) on published baseline | Exactly one `var<uniform>` to `var<storage, read>` change; CPU upload/readback host | 64 clustered particles ×4 steps, 128 ×2, 64 wrap/clamp particles ×1; 2,304 component comparisons; max absolute error 2.98e-8 |
| Same Boids compute on local rendering branch | Shader unchanged; real UNIFORM buffer | 128 clustered particles ×3 steps; 1,536 component comparisons; max absolute error 2.24e-8 |
| [Game of Life](https://github.com/webgpu/webgpu-samples/blob/e040ec1a20dbbe01afed30c812e825b2639cfe5b/sample/gameOfLife/compute.wgsl) | Shader unchanged; default blockSize=8, auto layout, upload instead of mappedAtCreation | Empty/full/block/blinker/wrap-glider/seeded grids, 8×8 through 32×16; 5,120 exact cell comparisons |
| [Bitonic sort](https://github.com/webgpu/webgpu-samples/blob/e040ec1a20dbbe01afed30c812e825b2639cfe5b/sample/bitonicSort/bitonicCompute.ts) | Exact upstream generator template at workgroupSize=8, then uniform-to-storage parameter change; CPU sequences flip/disperse stages | 16/32/64 elements, duplicates/reverse/unsigned extremes/equal/sorted inputs; 66 dispatches across separate runs; 2,304 exact per-stage comparisons; exact local atomic swap counts and final numeric sort |
| Offscreen triangle | Small test shader, no vertex buffers | Actual GPU render pass, 64×64 RGBA8 readback; 3,712 analytical pixel checks, 1,152 colored pixels |
| [Official Boids sprite shader](https://github.com/webgpu/webgpu-samples/blob/e040ec1a20dbbe01afed30c812e825b2639cfe5b/sample/computeBoids/sprite.wgsl) | Shader unchanged; offscreen host, one deterministic instance with angle zero | 3,712 analytical pixel checks, interpolated color within one UNORM8 unit; 1,152 colored pixels |
| Boids compute → render | Both official shaders unchanged; 64 grid particles, no neighbor forces, two submissions with CPU staging between them | Position error 7.45e-9; 256×256 GPU-rendered frame; 59,904 analytical pixel checks, 5,376 colored pixels |

Boids uses independent scalar CPU updates with double intermediates and f32 state
boundaries. The declared tolerance is `2e-5 + 2e-4 * abs(reference)` per component,
covering accumulation, normalization, and fused-arithmetic differences. Inputs
avoid zero-length velocity normalization. Counts are multiples of 64 because the
upstream shader has no explicit invocation bounds guard; arbitrary tail sizes are
untested. Boundary cases exercise speed clamping and all wrap directions.

Life tests use exact integer comparisons and power-of-two dimensions. These make
the shader's unsigned-coordinate underflow agree with mathematical toroidal wrap.
Non-power-of-two dimensions and partial workgroups are not covered. Bitonic tests
seed the output from the input at each phase because upstream global phases write
only swapped pairs. Shader logic is otherwise unchanged. The global phases do not
increment the counter; tests require zero there and exact swap counts locally.

Pixel references test geometry and shader color analytically. They exclude a
narrow edge band, where rasterization edge rules and floating-point differences
can matter. No CPU rasterizer supplies the output image. The PNG exporter merely
packages actual GPU readback bytes. These tests establish bounded correctness;
they are neither WebGPU conformance tests nor benchmarks.

## What the local implementation adds

Textures/views support one RGBA8 offscreen target, clear/store, triangle-list
render pipelines, vertex and instance inputs, a single draw, and padded texture
readback. Compute supports real uniform bindings. The official Boids compute and
sprite shaders therefore run unchanged, but its `main.ts` does not: the gallery
host still uses canvas/DOM, adapter options/features, optional queries, mapped
creation, and a combined compute/render loop absent here.

Rendering checks `operation: "render"` through the same GPU policy chain before
compiling shaders. Native checks bound source, dimensions, draw size, layouts,
and vertex bytes. Compute/render share the existing session/submission limits;
textures consume the existing cumulative device allocation budget. No policy,
OS sandbox, or hardening setting was loosened. The native worker cannot guarantee
preemption of an infinite shader or a blocked driver call.

## Testable next milestones

1. **Completed local slice:** offscreen triangle and unchanged Boids compute/render
   shaders, with numerical/pixel oracles and denied-render policy tests. Review
   the staged-data design and tests before treating this as broader API support.
2. **Native resource and command model:** replace per-submit reconstruction with
   bounded execution-local GPU resource handles; support ordered copies and
   multiple compute/render passes in one command buffer. Keep snapshot epochs,
   cleanup, policy checks, and total budgets. Test compute → render without CPU
   staging, cancellation, destroyed resources, and snapshot restore.
3. **Rendering coverage:** add sampler/texture uploads and sampled bindings,
   indexed draws, render bind groups, depth, and explicit pipeline layouts in
   bounded increments. Require an official textured example, an indexed mesh,
   depth overlap, and analytical pixels before claiming each capability.
4. **ONNX feasibility test:** choose and pin an ONNX Runtime Web release and a tiny
   Add/MatMul model, match its JS/WASM assets, verify a single-threaded bootstrap,
   then assert WebGPU provider execution and CPU-equivalent tensors. Fail the test
   on silent WASM/CPU fallback. Do not raise limits globally to fit larger models.

## ONNX source assessment, not an inference result

Inspected official ONNX Runtime source at commit
[`2927f3b3fcd2012394c8a4bb8d2d18f81cc0f768`](https://github.com/microsoft/onnxruntime/tree/2927f3b3fcd2012394c8a4bb8d2d18f81cc0f768),
including `js/web/lib/wasm/jsep/backend-webgpu.ts`, `webgpu/program-manager.ts`,
`webgpu/gpu-data-manager.ts`, `wasm-factory.ts`, and `wasm-utils-import.ts`.

The actual MCPJS probe uses V8/deno_core, not QuickJS. Under the test's default
ExecutionConfig, a minimal WASM module instantiated. Blob, URL, TextEncoder,
ReadableStream, SharedArrayBuffer and Atomics were present; Worker and unconfigured
fetch were absent, and crossOriginIsolated was false. Settings may change that
surface. This probe does not prove the ONNX SIMD WASM binary can load or run.

Concrete blockers in the inspected backend include adapter `limits`/`features`,
`requestDevice` descriptors, `device.lost`, mutable `onuncapturederror`,
`GPUValidationError`, optional error scopes, uniform ranges, copy-only command
buffers, and multiple dispatches per pass. The current frozen device object also
rejects ONNX assigning its error callback. Optional f16/subgroups/timestamp features
must be reported accurately; they are not prerequisites for every model.

[Official WebGPU setup](https://onnxruntime.ai/docs/tutorials/web/ep-webgpu.html)
requires the WebGPU build and explicit execution-provider selection. The
[environment documentation](https://onnxruntime.ai/docs/tutorials/web/env-flags-and-session-options.html)
provides a single-threaded configuration and matching WASM asset overrides.
Use `numThreads=1` and no proxy worker for the first headless attempt. Supply pinned
assets through existing authorized module/fetch paths or a tested binary override;
do not grant ambient network access. Existing 4 MiB allocation and 32-submission
limits may fit a tiny test but are not a general inference resource policy.
No ONNX package/model was installed or executed in this task.

## GPU-rendered WebM feasibility

The Mac has ffmpeg with libvpx VP8 and libvpx-vp9 encoders, Python, Node, and Pillow.
A future path can run GPU compute/render, read back RGBA frames, and encode those
frames to WebM. Encoding and image packaging may use CPU resources; geometry and
pixels must come from the GPU render pass. No WebM pipeline or video was built.

## Reproduction and evidence

Use the installed Nix Rust toolchain and the existing Cargo cache. GPU tests need
host GPU access: the executor sandbox alone exposed no adapter. Run one test thread
to avoid competing for the deliberately small global GPU session cap.

```sh
node --test tests/webgpu/shim.test.cjs
cargo test --offline -p server --lib engine::webgpu
cargo test --offline -p server --test webgpu --test compat_surface --test parameter_configs
cargo test --offline -p server --test webgpu --test webgpu_render_security -- --ignored --test-threads=1
cargo test --offline -p server --test webgpu_gallery -- --ignored --test-threads=1 --nocapture > gallery.log
python3 tests/webgpu/gallery/export_frames.py gallery.log output-frames
```

All 26 targeted tests passed: four JavaScript checks, two native bounds tests,
nine ordinary engine/regression tests, two existing hardware tests, one render
policy-denial test, and eight gallery/render/runtime tests.

Local evidence is retained beside the checkout in `../webgpu-validation/`:
`gallery.log` (published baseline), `render-gallery-final.log`, `render-unit.log`,
`render-security.log`, and GPU-readback `triangle.png`, `sprite.png`, `boids.png`.
Full Nix packaging, WebGPU CTS, other GPU backends, full browser sample execution,
and ONNX inference remain untested. The published branch remains unchanged.
