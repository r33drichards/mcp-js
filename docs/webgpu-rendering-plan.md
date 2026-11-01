# WebGPU gallery evidence and rendering milestones

The published compute baseline is `de267bb5dbd24177f733020d1a0fe12476a779c7`
on `feat/webgpu-compute`. This checkpoint is developed on `feat/webgpu-rendering-local`, based on the
published compute branch so its review diff isolates rendering. The implementation adds real
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
and ONNX inference remain untested. The compute baseline remains unchanged.


## Local persistent-buffer milestone

The tested rendering checkpoint remains `a71bff4980da270cd93b5fa57ebd7b3a8001478b`
on `feat/webgpu-rendering-local`. Publication of that checkpoint was blocked by
approval review; no push or PR was created. Subsequent work is isolated on
`feat/webgpu-resources-local` and has not been published.

Rust now owns a bounded execution-local registry of native buffers. The shim
passes private IDs and monotonically increasing host-upload revisions. Repeated
submissions with the same revision reuse native contents; a writeBuffer or host
copy advances the revision. Compute and render share this registry, so the Boids
output buffer is bound directly as vertex data without an intervening upload.
The native test deliberately supplies stale host bytes on a second dispatch and
requires the GPU's previous result, demonstrating that reuse is not merely a
numerically equivalent re-upload.

Policy gates still execute before native work. Existing session tokens, snapshot
rejection, one-in-flight restriction, 32-submission cap, object cap and cumulative
allocation limits remain. Native IDs are restricted to 1..128, aliases in a job
are rejected, immutable size/kind and monotonic revision are checked, and the
registry independently caps resident bytes at 4 MiB. JS destruction prevents
further use immediately; resident storage is reclaimed at device/session cleanup.
Pending native workers retain their Arc session until completion. No hard GPU
preemption guarantee is added.

This is buffer persistence, not persistent textures/pipelines or a complete command
model. Eager readbacks and CPU bytes in job serialization remain. Buffer copies
are still performed on the host. One pass/dispatch per command buffer remains;
combined compute/render, ordered native copies and multiple passes are the next
bounded command milestone. That milestone must charge total GPU work against the
existing limits, not multiply the work allowance by batching.

Thirty-one targeted tests passed: 5 shim checks, 3 ordinary native bounds checks,
1 native GPU identity/revision/budget test, 9 ordinary engine regressions, and
13 hardware integration/gallery/security tests. The new resident Boids case uses
both unchanged pinned shaders and the existing analytical pixel oracle. Logs:
`resources-unit.log`, `resources-native-gpu.log`, `resources-regressions.log`,
`resources-gpu.log` under the sibling validation directory. Run the additional
native hardware check with:

```sh
cargo test --offline -p server --lib engine::webgpu::tests::resident_identity_revision_and_budget -- --ignored --test-threads=1
```

ONNX bootstrap is still blocked before a defensible inference smoke test: the
pinned backend needs adapter limits/features, requestDevice descriptors,
device loss/error reporting, binding ranges, copy-only command encoders and
multiple dispatches per pass. Persistence alone does not satisfy these. A future
single-threaded test must pin matching WebGPU JS/WASM assets and a tiny model,
verify SIMD support and module loading under existing policies, and reject CPU
provider fallback. No ONNX assets were installed or inference executed.


## Local native command milestone

This follow-up is isolated on `feat/webgpu-commands-local`, after preserved
resource checkpoint `c35ee05d03f122093d42bda52c8e5a2d0c9f8ccf`. It records dispatch
state at each call, accepts multiple compute passes/dispatches and ordered
whole-buffer copies, and supports copy-only command buffers. Rust builds one
native command buffer and submits it once. Each JS dispatch becomes a native
compute pass; there is no intervening host readback between batch operations.
Resident resources are shared, and final readback updates the JS mirrors.

Per-dispatch binding count remains eight; the batch pool permits 16 distinct
buffers to include copy destinations, still within the same cumulative 4 MiB
resource budget. Aggregate batch source is capped at 64 KiB, with at most 32
dispatches and eight copies. Every dispatch is charged against the execution's
existing 32 work units; copy-only batches and render submissions each cost one.
The one-in-flight restriction remains. Native validation bounds indices, sizes,
usage flags and workgroups. Every dispatch and copy is authorized before any
native upload/compilation/work, so a denied later command cannot partially execute
the batch. Copies now require explicit `operation: "copyBuffer"` authorization;
the policy example has been updated. This adds a gate, not ambient GPU access.

Tests cover 257 float elements against `2*x+3` across pipeline switches, two JS
passes and ordered copies; copy-only resident reads and upload updates; 32 dependent
increments followed by work-budget rejection; pass lifecycle and cross-device
ownership; and later-dispatch/copy policy denial. The existing real-GPU Boids,
triangle, sprite, numerical gallery, snapshot and resource-budget tests still pass.
38 targeted tests passed: 7 shim, 4 native ordinary, 1 native GPU, 9 ordinary engine,
and 17 hardware integration/gallery/security tests. Evidence is in
`commands-unit.log`, `commands-native-gpu.log`, `commands-regressions.log`, and
`commands-gpu.log` in the sibling validation directory. Commands:

```sh
node --test tests/webgpu/shim.test.cjs
cargo test --offline -p server --lib engine::webgpu
cargo test --offline -p server --lib engine::webgpu -- --ignored --test-threads=1
cargo test --offline -p server --test webgpu --test compat_surface --test parameter_configs
cargo test --offline -p server --test webgpu_gallery --test webgpu --test webgpu_render_security -- --ignored --test-threads=1
```

The pinned ONNX SIMD capability probe validates and instantiates in the actual
MCPJS runtime; copied probe bytes match upstream and its MIT license is retained
as `tests/webgpu/gallery/upstream/ONNX-LICENSE`. A test reproduces the backend's
first failing adapter-limit access. ONNX remains blocked by missing adapter
limits/features and device descriptors/loss/error APIs. Its uniform binding has
explicit offset/size; pooled buffers may require partial copies, whereas this
slice accepts only equal-sized whole buffers. Matching JS/WASM assets, module
loading, a tiny model and verified provider selection still need a separate smoke
test. No inference or CPU fallback result is claimed.

Combined compute/render command buffers, persistent textures/pipelines, partial
copies/bindings, optional features and broad rendering coverage remain outside
this milestone. Full Nix packaging, CTS and other GPU backends remain untested.

After fresh approval, the single authorized push of frozen rendering commit
`a71bff4980da270cd93b5fa57ebd7b3a8001478b` passed approval review but GitHub rejected
it with GH007 (private commit email). A subsequent remote read confirmed that
`feat/webgpu-rendering-local` was absent. No retry, privacy-setting change, commit
rewrite or PR creation occurred. Publishing a replacement commit needs separate
scope approval; neither resource nor command follow-ups were approved for push.


## Local range prerequisite — independent review pending

`feat/webgpu-ranges-local` extends the preserved command checkpoint
`2012956979dfda50aa941b812d898198c29c8f33` with static storage/uniform binding
ranges and native partial buffer copies. This is locally tested, not yet
independently reviewed or declared ready. Binding offsets are restricted to
256-byte alignment; all sizes are positive multiples of four. Copies use distinct
buffers, four-byte-aligned offsets/sizes and checked end bounds, including unequal
allocation sizes. Zero-sized operations, aliases and dynamic offsets remain
unsupported. Native arithmetic uses checked addition to reject overflow.

The same allocation, object, source, work-unit and session budgets apply. Policy
`bytes` retains its full-allocation meaning; selecting a small subrange cannot
reduce that gate. Additional `bindingBytes`, `copiedBytes`, `sourceOffset` and
`destinationOffset` fields describe the selected region. Review should examine
native and JS bounds agreement, policy accounting, mutation/coherence after
partial operations, ownership/snapshot checks, and deferred handle cleanup.
No budget increase or policy bypass is introduced.

The real-GPU test binds 32 bytes of storage and 16 bytes of uniform data at offset
256 inside larger buffers. It checks `2*x+3` and preserves all surrounding
sentinels, then performs partial copies into unequal-sized allocations, including
an exact-end copy. Malformed ranges cover overflow, misalignment, zero size,
invalid indices and aliases. All 42 targeted checks passed (8 shim, 6 ordinary
native, 1 unchanged native GPU test, 9 ordinary engine, 18 GPU integration/gallery/
security checks). Range evidence is `ranges-unit.log`, `ranges-regressions.log`
and `ranges-gpu.log`; the unchanged native resident test was last run for the
command checkpoint. The same reproduction commands above apply.

### Exact first-inference plan

Pin `onnxruntime-web@1.23.2` from the official npm registry, release source commit
`a83fc4d58cb48eb68890dd689f94f28288cf2278`. Package integrity and planned artifact
names are recorded in `tests/webgpu/onnx-smoke-plan.json`. Unlike the earlier
moving-source assessment, this plan uses a concrete released build. The package
assets have not been downloaded or executed. Use its matching
`ort.webgpu.min.mjs`, `ort-wasm-simd-threaded.jsep.mjs` and
`ort-wasm-simd-threaded.jsep.wasm`; verify the recorded tarball integrity first.

Start with one Add node (opset 13, IR 8), two runtime float32 inputs of shape [4]
and one output. Inputs [1,2,3,4] and [10,20,30,40] must produce [11,22,33,44]
exactly; repeat with negative/fractional inputs and a fresh input upload to rule
out stale reuse. Disable graph optimization for this probe, use one WASM thread,
no proxy worker and only the WebGPU execution provider. Feed model bytes directly;
keep JS/WASM assets under the existing module/fetch allowlist or verified local
virtual-module/binary injection. Do not enable unrestricted network access or
assume the Node/browser loader works in MCPJS. Asset loading remains a separate
bootstrap test, under the current engine memory limit.

Expected path: V8 loads the pinned WebGPU JS and matching JSEP WASM; WASM creates
the model/session and orchestrates the Add kernel; ORT uploads tensors/uniforms,
binds pooled ranges, dispatches WGSL through the shim/native command buffer, then
copies/maps output back into the JS tensor. Acceptance requires model output,
actual GPU dispatch evidence linked to this run, and proof the Add node did not
execute through a WASM/CPU fallback. A matching tensor alone is insufficient.
Denial, bounds, resource lifetime and snapshot tests must remain passing.

The released backend still requires adapter `limits`/`features`,
`requestDevice({requiredLimits,requiredFeatures})`, device `limits`/`features`,
and assignable `onuncapturederror` with real validation-error delivery. Native
error/loss semantics must be implemented honestly, not placeholder callbacks or
never-resolving promises. Unlike the later source previously inspected, the
v1.23.2 initialization/dispose source here does not read `device.lost`; do not
mislabel it as this release's first bootstrap blocker. Debug error scopes and
optional timestamp/f16/subgroup paths can remain disabled for the first smoke,
with unsupported features accurately absent. Capability negotiation is the next
separate prerequisite; it must reflect native support and existing runtime limits.
No model execution or inference compatibility is claimed by this range work.

### Rendering publication receipt

After explicit metadata-only replacement approval, frozen rendering commit
`a71bff4980da270cd93b5fa57ebd7b3a8001478b` was preserved at
`checkpoint/webgpu-rendering-a71`. Replacement
`485a1429367a5e6f8cb4b46db222c84cba224774` has the identical tree
`09e4d295d88a797d4b82b126c2647348ec759285` and unchanged parent, with the verified
GitHub noreply identity. No privacy settings were changed. The remote branch
`feat/webgpu-rendering-local` matches that replacement; draft PR
https://github.com/r33drichards/mcp-js/pull/308 targets `feat/webgpu-compute`.
Resource, command and range follow-ups remain local and are not in that PR.
The last observed Module Policy Integration check was still in progress.


## Independent review fixes — awaiting re-review

Review of `d7a97dbee769168de55ebdf777a8e67e3485757f` identified two actionable
issues. Both were reproduced on the real GPU before changes, with failing logs
`review-p1-before.log` and `review-p2-before.log`. Fixes are isolated on
`feat/webgpu-review-fixes-local`; all earlier checkpoints remain preserved.

**P1 — destination allocation omitted from copy policy.** A four-byte source
copied into a 2 MiB destination passed `input.bytes <= 1048576`, because the
metadata counted only the source. Copy `bytes` now sums both complete allocations;
`sourceBytes` and `destinationBytes` expose each component, while `copiedBytes`
remains the requested range length. This cannot relax an existing byte check.
Equal-sized copies now report twice one buffer's size, an intentional stricter
policy behavior. Native unit checks cover both asymmetric directions, and the
real-GPU policy regression requires denial for the small-source/large-destination
case before native allocation/upload/readback work.

**P2 — usage classification changed across paths.** `UNIFORM | VERTEX` was accepted
but registered as a boolean uniform kind in compute/copy and as non-uniform in
render. Compute-then-render reproduced the native identity mismatch. The registry
now stores immutable complete declared usage masks; every path passes that mask,
and native creation preserves its declared GPU role union, plus internal staging
copy capabilities. MAP_READ stays backed by separate staging buffers. Render
explicitly requires VERTEX usage, and a shared validator enforces legal flags,
alignment, size and uniform limits. Existing IDs cannot change even a non-uniform
usage bit. This supports the accepted union instead of rejecting it after use.

The real-GPU regression verifies uniform value -0.5 and rendered triangle pixels
in four fresh executions: compute→render, render→compute, copy→compute→render,
and copy→render→compute. Both focused tests now pass (`review-p1-after.log`,
`review-p2-after.log`). Full verification passed 45 targeted checks: 8 shim,
7 ordinary native, 1 native GPU, 9 ordinary engine and 20 GPU integration/gallery/
security tests. Logs are `review-fixes-unit.log`, `review-fixes-native-gpu.log`,
`review-fixes-regressions.log`, and `review-fixes-gpu.log`. Commands:

```sh
node --test tests/webgpu/shim.test.cjs
cargo test --offline -p server --lib engine::webgpu
cargo test --offline -p server --test webgpu --test compat_surface --test parameter_configs
cargo test --offline -p server --lib engine::webgpu -- --ignored --test-threads=1
cargo test --offline -p server --test webgpu_gallery --test webgpu --test webgpu_render_security -- --ignored --test-threads=1
```

No allocation/work budget was increased, and the published rendering PR was not
changed. ONNX bridging remains paused until independent re-review integrates.
These are locally validated fixes, not an independent approval or a conformance
claim; existing inference and full-Nix-packaging limitations remain.


## Native capability profile and verified asset probe

After independent re-review cleared `c0e643335b458d3bc0176d861886f7524e96f292`,
work continued locally on `feat/webgpu-capabilities-local`. Native-backed adapter
info and adapter/device limits are exposed as immutable objects. Info uses actual
wgpu vendor/device IDs and description; architecture is unknown and explicitly
empty. The feature set is empty because the existing native device enables no
optional features, with a native assertion guarding that invariant.

The runtime still creates one fixed-profile device during adapter acquisition.
`requestDevice` now validates descriptors natively: only empty requested features
and supplied limits exactly equal to the existing device's actual limits are
supported. Lower/higher limits are rejected rather than pretending to negotiate
a different device. This is an explicit subset, not full WebGPU negotiation.
Native limits may exceed the separate JS allocation/render/work quotas; none of
those quotas were raised. Adapter power preference/fallback options are validated
and passed to wgpu's actual request, after the existing policy gate; that gate now
also receives those selection fields. Forced fallback availability itself remains
backend-dependent and was not demonstrated on this Mac.

Capability/selection/descriptor tests and existing regressions pass: 48 targeted
checks (9 shim, 8 ordinary native, 1 native GPU, 9 ordinary engine, 21 GPU
integration/gallery/security). Evidence is `caps-unit.log`, `caps-native-gpu.log`,
`caps-regressions-serial.log`, and `caps-gpu.log`. The initial parallel ordinary
regression run (`caps-regressions.log`) failed `test_fast_computation_succeeds`
and exited with SIGSEGV during the parameter suite, without an assertion
diagnostic or stack establishing the cause. The serial rerun passed all
nine ordinary checks; the parallel failure is retained and not resolved by this
capability milestone. The independent reviewer has not yet
reviewed this new capability diff. No error callbacks, device-loss promises or
error scopes were faked or added in this milestone.

### Corrected published asset pairing and actual bootstrap result

The official `onnxruntime-web@1.23.2` tarball was downloaded through the reviewed
path and its pinned SHA-512 verified. Only selected assets were extracted; no
package installation scripts or toolchain installation ran. The initial asset
plan contained a mismatch: shipped `ort.webgpu.min.mjs` references Asyncify and
expects `webgpuInit`, while the selected JSEP factory exposes `jsepInit`. That
mismatch produced `webgpuInit is not a function`; it was a harness mistake, not
a missing MCPJS API. The matching published JSEP combination is now:

- `ort.all.min.mjs` (explicit `executionProviders: ['webgpu']`)
- `ort-wasm-simd-threaded.jsep.mjs`
- `ort-wasm-simd-threaded.jsep.wasm`

Exact SHA-256 hashes and package integrity are recorded in
`tests/webgpu/onnx-smoke-plan.json`. The built all entry point includes the JS
WebGPU backend corresponding to the source assessment. The dedicated WebGPU
entry point's different native/Asyncify implementation is not covered here.

`server/tests/onnx_bootstrap.rs` registers only the verified JS sources as virtual
modules, keeps external imports disabled and supplies verified WASM bytes with
`env.wasm.wasmBinary`. The heap stays at 128 MiB. A single large base64 decode
exceeded that heap; chunked decoding in the existing bootstrap hook resolved that
transport issue. Temporary console capture uses the same supported configuration
as the gallery tests. The generated Add graph remains a candidate, not a
runtime-validated model, because backend initialization fails before execution.

The matched-assets probe now fails at the real missing API:
`TypeError: Cannot add property onuncapturederror, object is not extensible`.
See `onnx-bootstrap-jsep-matched.log`. It passes asset loading, adapter selection,
native capability reads and exact-profile device validation to reach that point.
No output tensor, GPU inference dispatch or CPU-fallback-free execution has been
verified. The ignored `pinned_add_smoke` intentionally remains a failing
experimental smoke; it also refuses to treat matching output alone as verified
GPU inference. It is separate from the 48 passing regression checks.

Reproduce with the verified local asset directory:

```sh
MCP_ONNX_ASSET_DIR=/absolute/path/to/onnx-1.23.2-assets cargo test --offline -p server --test onnx_bootstrap -- --ignored --test-threads=1
```

The next scoped bridge must deliver typed native validation/OOM/internal errors
and actual device loss through bounded execution-local state, without indefinitely
keeping the event loop alive or fabricating success on overflow/cancellation.
Current wgpu has real uncaptured-error/device-loss callbacks, but choosing and
reviewing their queue/lifetime behavior remains necessary before implementation.
No further bridge or publication occurred in this milestone.

## Native callback prototype (not connected to the runtime)

`server/tests/webgpu_callback_lifecycle.rs` exercises actual wgpu callbacks with
weak ownership: callbacks do not retain the collector or a Session/device cycle.
The prototype bounds queued errors to 16 and formatted messages to 1024 UTF-8
bytes, records overflow explicitly, and retains the first device-loss notification
in a separate slot even if the error queue is full. Closing clears retained events
and ignores late callbacks. Two ordinary tests and one real-GPU test passed
(`lifecycle-unit.log`, `lifecycle-gpu.log`). Real GPU evidence covers invalid-WGSL
validation and explicit device destruction, not spontaneous driver loss or OOM;
OOM/internal classifications are synthetic unit tests.

This is a test-only prototype, not an installed handler or a JS API change. Next
integration requires a single bounded, unreferenced async receiver per session,
execution teardown wakeup, typed JS delivery and loss resolution. Queue overflow
must fail the session explicitly rather than silently dropping errors or inventing
native loss. Those integration semantics still need independent review. Existing
native error scopes consume operation errors and return rejected submissions;
uncaptured callbacks must not duplicate these as separate events.

The parallel parameter test suite was rerun with `--nocapture` and all six tests
passed (`lifecycle-parameters-parallel.log`). The original failure remains
unresolved: its log records a failed test and SIGSEGV without an assertion detail
or stack. A timing cause is **not established**. These tests configure no GPU, but
that alone does not establish causality or rule out a capability-related effect.
Serial success and this parallel success do not resolve intermittent stability.

## Effective capability limit correction

Independent review found that the original capability checkpoint exposed raw
native limits through standard adapter/device fields even when the shim could not
perform the corresponding operations. This supersedes that checkpoint's raw-limit
contract. Both fields and request validation now use the same effective profile:
min(native capability, implemented per-object ceiling), with zero for unsupported
1D/3D textures, sampled/storage texture bindings, samplers and dynamic bindings.
Bind groups are capped at one, entries at eight, buffer/storage binding size at
4 MiB, 2D dimension at 1024, array layers/color attachments at one, and vertex
buffers at four. Native stage-specific binding limits can reduce these further.
Binding alignment is at least the shim's 256 bytes. No resource budget increased.
Raw limits are not exposed through a second JS API.

These per-object limits coexist with cumulative 4 MiB/session allocation, draw,
dispatch product and submission quotas. They do not promise all combinations can
be allocated simultaneously or that unsupported formats/usages are available.
This experimental subset reports zero for unsupported categories rather than
claiming WebGPU's baseline conformance limits. Exact-profile-only request
validation remains the documented subset; general limit negotiation is absent.

Validation: eight native unit checks and 23 GPU integration/gallery/security
checks passed (`effective-unit.log`, `effective-gpu.log`). The two new boundary
checks also passed after strengthening the texture check to actual render/readback
(`effective-boundaries-gpu.log`). They dispatch at the advertised storage binding
count through group zero, reject group one, dispatch a full advertised 4 MiB
storage binding, and render/read pixels at the advertised 1024-pixel texture width.
Existing numerical Boids and rendering/policy/ownership regressions passed. These
are representative consistency tests, not exhaustive capability conformance.

Controlled crash check: an immutable `git archive` of pre-capability
`c0e643335b458d3bc0176d861886f7524e96f292` was built in a temporary directory
with the same Nix Rust/CARGO_HOME and shared build cache. Running
`RUST_BACKTRACE=1 cargo test --offline -p server --test parameter_configs -- --nocapture`
passed all six parallel tests (`precap-parameters-parallel.log`), as did the current
capability checkout's parallel rerun. The failure did not reproduce, so no new
assertion diagnostic or crash stack was obtained. This does not establish the
cause or resolve the original SIGSEGV; the initial log remains retained.

Commands for the effective-limit evidence, using the installed Nix Rust and
`CARGO_HOME=/tmp/mcp-webgpu-cargo`:

```sh
cargo test --offline -p server --lib engine::webgpu
cargo test --offline -p server --test webgpu_gallery --test webgpu --test webgpu_render_security -- --ignored --test-threads=1
cargo test --offline -p server --test webgpu_gallery effective_limits -- --ignored --test-threads=1
```

## Native/runtime/JS event integration

The bounded collector is now connected to Session and the JS device. Native
callbacks capture only Weak collector references; one receiver is permitted at a
time, cancellation releases its slot, and closing clears events and wakes it.
GpuState teardown closes and destroys the device even when a submitted worker
still retains Session. Session drop is also idempotent cleanup. Native work checks
terminal state before starting; already submitted work has no hard preemption.

The async receive op validates the session token again after draining. A separate
native delivery check verifies the current token, overflow and closed state before
JS delivery and each handler invocation; a previously drained batch is not a
capability to deliver after teardown/overflow. JS readback completion also checks
that the device is still live. Native loss is retained independently of errors.
There is one unreferenced op per device, not a polling timer; it does not retain
Session. Merely awaiting the loss promise cannot keep an otherwise idle execution
alive. Runtime teardown discards JS context and closes/wakes native receivers.

The JS subset exposes one onuncapturederror callback, three correctly classified
GPU error types, and device.lost. Explicit destroy resolves loss after actually
calling native destroy; native loss resolves from the callback record. Queue
overflow/receiver failure/throwing handler terminates the session and rejects
lost, rather than fabricating a native loss. This exception behavior is an explicit
subset limitation; no full EventTarget or JS error-scope API is claimed. Existing
native scopes capture submission errors without duplicating callbacks. Message
bounds apply to retained text, not upstream formatting CPU time/allocation.

Validation evidence (61 targeted checks):

- 13 shim tests (`events-shim.log`), including stale drained delivery, destruction
  inside a handler, typed errors, native loss and terminal overflow.
- 12 ordinary native checks (`events-final-delivery-unit.log`), including close
  wakeup, cancelled/duplicate waiter handling and invalidating drained events.
- Three native GPU checks (`events-delivery-native-gpu.log`), including real
  invalid WGSL through the entire native callback -> V8 handler path and actual
  native destruction -> lost resolution, with bounded waits.
- 24 GPU integration/gallery/security checks (`events-final-gpu.log`), including
  idle runtime exit with an undestroyed device, previous snapshots/policy/budget
  regressions, numerical Boids and GPU rendering.
- Nine serial ordinary regressions (`events-ordinary-serial.log`). The earlier
  parallel SIGSEGV remains unresolved, not erased by these passing checks.

Two test-harness failures are preserved: `events-native-gpu.log` dropped the queue
early and did not receive a destruction callback; retaining the queue, as runtime
Session does, passed. `events-native-runtime-gpu.log` tried to await an unreferenced
receiver in an otherwise idle loop and got PendingPromiseResolution; a short
referenced timer in the delivery test provides an active loop, while the separate
idle-exit test verifies that no event receiver holds execution open. Neither
failure was fixed by making the production receiver referenced. Real spontaneous
driver loss and real OOM/internal failures remain untested (classification and
overflow use synthetic tests).

Pinned ONNX probe `onnx-bootstrap-events-session.log` now prints
`ONNX_PROBE_SESSION_CREATED`: matching official 1.23.2 JSEP assets successfully
create the tiny Add session. session.run then fails in input upload with
`webgpu: mappedAtCreation is unsupported`. No output, inference dispatch or
CPU-fallback-free inference is verified. The ignored smoke remains failing and
separate from the passing regressions. Its assets/model/hashes are unchanged;
only a session-created marker was added. The next prerequisite is mapped-at-
creation upload/unmap semantics within existing quotas, after independent review
of this event integration. No publication or budget/security relaxation occurred.

Reproduction uses the installed Nix Rust and CARGO_HOME from prior sections:

```sh
node --test tests/webgpu/shim.test.cjs
cargo test --offline -p server --lib engine::webgpu -- --test-threads=1
cargo test --offline -p server --lib engine::webgpu -- --ignored --test-threads=1
cargo test --offline -p server --test webgpu_gallery --test webgpu --test webgpu_render_security -- --ignored --test-threads=1
cargo test --offline -p server --test compat_surface --test parameter_configs --test webgpu -- --test-threads=1
MCP_ONNX_ASSET_DIR=/absolute/path/to/onnx-1.23.2-assets cargo test --offline -p server --test onnx_bootstrap -- --ignored --test-threads=1
```

## Destroy preserves authoritative terminal outcomes

Independent review of frozen `80b746d9852778989d2b705d9d531ccd9436327a`
found that explicit destroy could overwrite queued or already-drained unknown loss
with `destroyed`, or incorrectly resolve a prior overflow as successful destruction.
The collector now closes and snapshots its terminal outcome under one mutex.
Closing clears queued errors but retains the first loss/overflow records until
collector release; draining never consumes these records. Late callbacks cannot
replace the outcome. If loss occurs first, subsequent errors cannot overflow the
queue; if overflow occurs first, it remains authoritative even if a later native
loss is also recorded.

The native destroy op returns that atomic outcome after removing the session and
calling device.destroy. JS uses one settlement path for explicit destroy, native
loss and receiver failure: prior overflow rejects, prior native loss resolves with
its original reason/message, and only destruction without an earlier terminal
outcome resolves as `destroyed`. A stale native destroy rejects instead of claiming
a successful native destruction. Existing delivery-time validation and receiver
teardown remain intact; no new resource budget or permissions are introduced.

Fresh validation: 17 shim checks, 14 ordinary native checks, four native GPU
checks and 24 GPU integration/gallery/security checks passed (59 total). Evidence:
`terminal-outcome-shim.log`, `terminal-outcome-unit.log`,
`terminal-outcome-native-gpu.log`, `terminal-outcome-gpu.log`. New cases cover both
queued and already-drained unknown loss/overflow followed by destroy, repeat close,
and first-terminal ordering. The native regression calls the actual destroy op
through V8, on real devices, verifies its returned JSON and session invalidation.
Unknown loss and overflow are injected test-only records: this is not a claim to
have induced spontaneous driver loss or real OOM. Existing real invalid-WGSL and
native-destruction-to-JS tests still pass.

Commands are the previous section's shim, native unit, native GPU and GPU-gallery
commands. No mappedAtCreation changes, ONNX reattempt, publication, or unrelated
crash retries were made in this fix. The ONNX upload blocker and unresolved earlier
parallel SIGSEGV remain unchanged. This fix is ready for independent re-review.


## Mapped initialization checkpoint (local, 2026-11-01)

Based on reviewed `81957f5a4e673b0d004cf919fb50ade268abff3b`, this adds
`mappedAtCreation` initialization and MAP_WRITE|COPY_SRC upload staging. Initial
bytes are zeroed; one full-buffer mapped range aliases the charged backing store.
Unmap transfers that store, detaches the caller's ArrayBuffer and advances upload
revision. Buffer/device teardown detaches tracked ranges. Usage remains immutable,
MAP_READ/MAP_WRITE combinations are validated in JS and native code, and existing
cumulative allocation/object budgets and snapshot ownership rules are unchanged.
General mapAsync WRITE, partial/multiple ranges and multiple submissions in flight
remain unsupported. Native buffers strip both mapping flags because mapping uses
separate host staging; copies and shader dispatch still execute through wgpu.

Validation: 20 shim, 14 ordinary native, 4 native GPU, 4 GPU integration,
20 gallery and 1 rendering-policy checks passed (63 total). Commands:

```sh
node --test tests/webgpu/shim.test.cjs
cargo test --offline -p server --lib engine::webgpu -- --test-threads=1
cargo test --offline -p server --lib engine::webgpu -- --ignored --test-threads=1
cargo test --offline -p server --test webgpu_gallery --test webgpu --test webgpu_render_security -- --ignored --test-threads=1
MCP_ONNX_ASSET_DIR=/path/to/verified/assets cargo test --offline -p server --test onnx_bootstrap -- --ignored --test-threads=1
```

The last command intentionally fails at the next actual compatibility gap:
`webgpu: one command buffer and one submission in flight supported`, in ORT
1.23.2 upload after `ONNX_PROBE_SESSION_CREATED`. No Add output, GPU inference
or no-fallback result has been verified. ORT submits input uploads without awaiting
each completion and destroys upload staging after submission. The next milestone
must review bounded ordered queue admission and submitted-resource lifetime;
removing the pending guard alone is not a correct fix.

The new real-GPU test initializes [11,22,33,0], detaches the mapped range, copies
to storage, dispatches addition and verifies [12,23,34,1]. Existing numerical
Boids/Life/bitonic and real rasterization, policy/budget/ownership regressions pass.
Logs outside Git in ../webgpu-validation: mapped-final-shim.log,
mapped-trace-resident-unit.log, mapped-trace-resident-native-gpu.log,
mapped-trace-resident-gpu.log (four integration passes, then one stale gallery
error-message expectation), mapped-final-gpu.log (all gallery/security pass after
correcting that assertion), and onnx-mapped-final.log. The initial native usage
assertion also incorrectly rejected newly valid MAP_WRITE; corrected before the
passing native run. Failed runs remain preserved as evidence.

Mac build recovery is confirmed. Required source manifests and Rust cache inputs
were File Provider dataless placeholders. Bounded Cargo/process diagnostics
identified blocked metadata/proc-macro inputs, including librustversion. Exact
required files were hydrated using Foundation FileManager's public download API;
no source restore, global sync changes, cache deletion or installation occurred.
Recovered native compilation/tests completed in 16.15 seconds; subsequent gallery
and ONNX probes completed normally. Original dirty source backups remain preserved.

The unrelated earlier parallel suite SIGSEGV remains unresolved; later passes do
not establish its cause. Full Nix packaging, other GPU backends, CTS, spontaneous
loss and real OOM remain unverified. This local mapping checkpoint needs independent
review before further resource/queue semantic changes. PR #308 contains the reviewed
81957f5 tree under public commit afda96bcb9126fe7af3a6b9335438f1bd1771581;
this new mapping checkpoint is not part of that publication.

## Mapped-range property shadowing fix (local, 2026-11-01)

Independent review found that caller-defined `byteLength` properties could skip
mapping detachment or throw before native device destruction and lost settlement.
Mapping now uses captured intrinsic ArrayBuffer length and transfer operations.
The first native regression also exposed the structuredClone compatibility shim's
own property lookup, so WebGPU mapping no longer routes through that shim.
Detachment clears tracking in finally; device cleanup catches per-range failures,
continues other ranges, always attempts native teardown and settles lost using
existing authoritative native terminal-outcome precedence. A cleanup failure with
no prior native terminal outcome rejects lost; it cannot report successful cleanup.

Validation: 27 shim tests and 26 real-GPU integration/gallery/security tests pass.
Six shadow-property cases cover throwing getters and zero values across unmap,
buffer destruction and device destruction, with actual native session invalidation.
A synthetic transfer failure verifies later mappings are cleaned, native destroy
runs, repeat destroy is harmless and lost rejects. Existing GPU initialization,
numerical/rendering/policy tests pass. Commands are the preceding checkpoint's
node shim and combined GPU-gallery Cargo commands. Logs: mapped-shadow-shim.log,
mapped-shadow-intrinsic-gpu.log; initial structuredClone failure retained in
mapped-shadow-gpu.log. No queue implementation or publication occurred.

A post-test Git diff exited with signal 10 (status 138); a subsequent diff completed
successfully after a delay. Cause is unestablished. This is distinct evidence from
the earlier unresolved test-suite SIGSEGV, whose relationship to WebGPU remains
unestablished. Source backups are retained under /tmp/mcp-mapped-shadow-ge5v04g6.

## Bounded FIFO checkpoint (local, 2026-11-01)

Developed only in the recovered non-iCloud checkout, based on recovery commit
`8b0e0d6ae0dbcb6c7d961e74ef1391f4200f1dcc`. One command buffer is accepted per
submit and exactly one native operation runs at a time. Admission validates all
resources, reserves max(1, dispatch count) from the unchanged cumulative 32-unit
allowance, and consumes the command synchronously; rejected admission does neither.
Accepted units are never refunded. Active plus waiting jobs are bounded by 32 job
records, 128 buffer references and 64 KiB of UTF-8 serialized command metadata.

The FIFO captures immutable command metadata and usage while retaining existing,
already-charged backing records. It does not retain a fresh 4 MiB snapshot per job.
Only the active job serializes bytes/revisions, after preceding readback updates.
Destroying a public buffer forbids future use but retains its backing for admitted
jobs; its last queued use releases destroyed host storage. Native resident storage
still follows the unchanged session cleanup and cumulative allocation rules.
Compute and render can be queued in order; render metadata is captured before
texture wrapper destruction. Public writeBuffer remains blocked while any work
is outstanding. This is a deliberate subset restriction, not full WebGPU ordering.

Mapping and onSubmittedWorkDone capture the current admitted prefix. Pending maps
prevent later admission of their buffers but do not invalidate earlier jobs.
Failures cancel the entire remaining suffix and terminate the native session
conservatively, including policy/validation rejection; fences settle once and an
earlier fulfilled prefix stays fulfilled. Native first-loss/overflow precedence
remains authoritative. Device teardown cancels waiting jobs immediately; already
submitted driver work retains the existing no-hard-preemption limitation.

Every native job still validates full allocation sizes and policy for every
operation before uploading/compiling/submitting. Copy policy counts complete source
and destination allocations. Native operations now revalidate token/configuration
and terminal state after asynchronous authorization and before result delivery;
JS also checks before starting and before applying readback. The event receiver's
separate terminal-record delivery path is unchanged.

Validation: 88 targeted checks pass: 38 shim, 14 ordinary native, five native GPU,
29 GPU integration/gallery/security, and two default-disabled/adapter-policy tests.
The real native boundary test uses cfg(test)-only scheduling barriers after policy
and after readback: teardown before the former leaves the native registry empty;
teardown at the latter occurs after two native buffers were created/copied, and
both cases reject stale delivery. The barriers add no production API. Controlled
shim tests cover pending-operation teardown, prefix fences, stale delivery, native
loss/overflow cancellation, metadata UTF-8/reference limits, and command/work
reservation. Real GPU tests cover immediate staging destruction, Float32 addition
[11,22,33,44], later write/remap [12,24,36,48], queued policy denial, and unchanged
Boids compute/sprite shaders with ordered offscreen rendering and early wrapper
destruction. The hand-written GPU Add is a queue regression, not ONNX inference.

Commands (installed Nix Rust, CARGO_HOME=/tmp/mcp-webgpu-cargo,
CARGO_TARGET_DIR=/tmp/mcp-webgpu-rederived-target, dev/test debug info disabled):

```sh
node --test tests/webgpu/shim.test.cjs
cargo test --offline -p server --lib engine::webgpu -- --test-threads=1
cargo test --offline -p server --lib engine::webgpu -- --ignored --test-threads=1
cargo test --offline -p server --test webgpu_gallery --test webgpu --test webgpu_render_security -- --ignored --test-threads=1
cargo test --offline -p server --test webgpu -- --test-threads=1
MCP_ONNX_ASSET_DIR=/tmp/mcp-fifo-onnx-assets cargo test --offline -p server --test onnx_bootstrap -- --ignored --test-threads=1
```

The last command remains a failing probe, outside the 88 passes. Official ORT
1.23.2 package SHA-512 and all three matching all/JSEP asset SHA-256 hashes were
reverified after download into /tmp; no installation scripts ran. The actual probe
logs ONNX_PROBE_SESSION_CREATED, passes the former FIFO admission blocker, then
fails Add4 with `webgpu: buffer unavailable for upload`. The pinned official
package source map identifies `../lib/wasm/jsep/backend-webgpu.ts:609`: ORT creates
a COPY_DST|UNIFORM buffer and calls queue.writeBuffer while submitted input work
is outstanding. Provider configuration is still ['webgpu']; no CPU fallback was
enabled. No ORT output or GPU inference dispatch is verified. Ordered writeBuffer
support needs a separately reviewed scope; the pending guard was not weakened.

Evidence in /tmp: mcp-fifo-shim.log, mcp-fifo-final-unit.log,
mcp-fifo-native-boundaries.log, mcp-fifo-reviewed-gpu.log,
mcp-fifo-reviewed-default-policy.log and mcp-fifo-final-onnx.log. Earlier failed
compile evidence remains mcp-fifo-unit.log (op-macro wrapper called as a Rust
function; corrected to a dedicated native helper). Full Nix packaging, CTS,
other backends and spontaneous driver-loss/OOM remain untested. The original
SIGSEGV and Git signal-10 causes remain unestablished. No publication, original
checkout/lock changes or broader writeBuffer implementation occurred.
