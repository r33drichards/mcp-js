# Verification and remaining work

Inspected/build base: fe5b613db0afcc9cf20d7ca1ef659c914a0b5008. Run date: 2026-10-23. No model calls, paid services, production access or external messages.

| Check | Actual result | Scope/limit |
|---|---|---|
| Offline Rust build from current source | Passed | Three deprecation warnings; preserved binary outside source |
| Three-node seed, policy denial, isolated writes | Passed | Disposable local S3-compatible PUT/GET fixture |
| Clean merge, conflict, stale-head CAS, label reset | Passed | See evidence/core-test-results.json |
| Leader kill/restart | Passed | Only task-owned labeled node; storage fixture stayed available |
| Majority loss/recovery | Passed | Timed-out label write was observed committed later; not continuous write availability |
| Official Grafana MCP through mcp-js | Passed | Earlier synthetic fixture, actual query response; not Astronomy |
| COW file export and Grafana provisioning | Passed | Earlier fixture; explicit separate bridge |
| mcp-js → original computer-use browser_execute | Passed | Headless local Chromium adaptation, preserved Host/Origin guard; actual screenshot has plotted samples |
| Full desktop Nix browser build | Blocked | Build cannot resolve cache.nixos.org/github.com; host-network retry could not resolve configured proxy. No access-control change attempted |
| Pi 1.4 session fork + native MCP | Passed | Real SessionManager files, MCP handshake/list/call, independent root, override rejection, abort, merge; no interactive hook dispatch or model |
| Python compilation | Passed | All demo Python modules |
| Base Compose with browser/agent profiles | Passed config validation | Not equivalent to launching all services |
| Generated Astronomy Compose | Passed config validation | Pinned 2.1.3; actual workload not started |
| Astronomy/Loki error logs and metrics | Unverified | Requires full workload, nonempty telemetry, actual flag reload |
| Pi agent → Astronomy → dashboard → browser | Unverified | No model credentials/calls configured; no end-to-end success claimed |

Disk reclamation was authorized separately. Before: 4,265,902,080 bytes free. Removed only rebuildable compiler dependency/incremental caches, task image rootfs copy, task npm cache and one unused task image. After: approximately 4.6 GiB free. The pre-existing compiler files mostly occupied the lower filesystem layer, so removing them did not recover their apparent size. `/tmp` is RAM-backed (8.8 GiB); it is not a suitable larger persistent Docker image store. No unrelated resources, source/git, journals, volumes or credentials were deleted. See evidence/disk-reclamation.json.

Full startup is blocked by the 14 GiB free-space preflight. This is a resource blocker, not a denied action. The original browser build is independently blocked by build-network DNS. A Linux rehearsal host with sufficient space and working build networking is needed. Pi model execution additionally requires the user's existing approved setup; no credentials were generated.

The recorded screenshot explicitly shows an earlier **synthetic component test**, not Astronomy Shop, not a historical dataset repair, and not an agent recording. Do not present it as a complete live demo. The complete repository unit/integration suite was not rerun; the focused disposable tests above are the actual executed tests.

Mac handoff: ownership-checked docker-exec HTTP transport passed a read-only three-node status check in Linux Docker. Portable source-build Dockerfile and explicit amd64 Compose selection are prepared; Mac build/emulation and full runtime remain unverified and assigned to the local task.

Narrated companion update: contiguous 00–45 slide schedule, companion dialog, explicit missing-media status, disabled playback without media and no browser JavaScript errors passed. Manim/Pango installation and official-image extraction failed; Phonon runtime installed but model metadata access returned HTTP 403. No speech or Manim sample was generated. Detailed reproduction and blockers are in animation/README.md.

Manim follow-up: staged official Debian development headers resolved the Python build. Five silent 480p chapters rendered and were probed; total MP4 size 977,002 bytes. Representative render took 4.3 seconds and ~315 MiB peak RSS; inspected route/fork frames have no clipping. These are explanatory previews without audio, not live recordings. Phonon speech synthesis remains blocked by model HTTP 403.
