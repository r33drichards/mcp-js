# Interview walkthrough and disposable demo

Start with [slides.html](slides.html) (self-contained, arrow-key navigation, N presenter notes, A agenda) and [presenter-guide.md](presenter-guide.md). Audience: interviewers evaluating Robert’s work; challenge brief unknown. Exactly 45 minutes including six minutes Q&A.

**Status:** current mcp-js core and several integration components were exercised. The complete Astronomy Shop → logs/metrics → paired Pi investigation → dashboard/browser flow has **not** passed end to end. Read [verification.md](verification.md) before presenting anything as live. No model calls or credentials were used.

## Prerequisites and startup

Linux x86_64; Docker Engine + Compose v2; Python 3 with PyYAML; a trusted Linux mcp-js `server` binary built from base `fe5b613`; Git; network access to pinned public sources/images during preparation. Reserve at least 14 GiB free disk and 8 GiB RAM (more for the full desktop browser). This cloud environment has only about 4.6 GiB free after scoped cleanup. Do not run the full stack until the disk gate passes. On macOS the controller uses ownership-checked docker exec HTTP requests, avoiding Docker Desktop private-IP routing. On Linux this transport is opt-in with MCP_JS_DEMO_TRANSPORT=exec. The portable build and full Mac runtime remain unverified.

From this directory:

```sh
./launch.sh prepare --binary /absolute/path/to/current/server
./launch.sh up
./launch.sh serve
```

Preparation fetches exact source commits, renders the upstream Astronomy Compose model, removes all original published ports and collector host/socket mounts, reuses its instrumentation/Prometheus/OpenSearch/Jaeger, replaces Grafana with the local Viewer instance, and adds Loki OTLP export for the official Grafana MCP log tool. It builds only the local runtime image from the supplied binary. `up` resolves external image tags into `.state/images.lock.json` digests before startup and builds pinned-source browser/OpenSearch images. A build or download failure is a hard setup failure, not a live fallback. There is no Docker socket inside the agent/browser/controller containers; the narrow controller runs on the host.

Generated `.state/compose.json` is the full integration; `compose.astronomy.json` is a portable checked-in full reference with real service names; `compose.yml` is the small component-test template used by the generator, not a claim that the old replay is Astronomy Shop. Source pins and adaptations are in `.state/sources.json`. The added log backend is **Loki**, while stock Astronomy **OpenSearch** remains intact. Only synthetic local workload data is allowed. The production computer-use gVisor boundary is not reproduced by Docker.

Open slides at `http://127.0.0.1:38080`, Grafana at `http://127.0.0.1:38300`, and Astronomy at `http://127.0.0.1:38880`. Ports bind loopback only; in this execution environment internal Docker networks did not publish ports, so host verification used owned container private IPs. Verify accessibility on the actual rehearsal machine.

The generated query service depends on the browser TCP readiness check at shared loopback 8081. Wait until all health checks pass and query is healthy. Full-stack readiness behavior remains unverified here.

```sh
python3 demo.py seed
python3 demo.py policy
python3 demo.py query
python3 demo.py logs
# After nonempty telemetry, operator only:
python3 operator-fault.py on
# Wait for actual metric and log evidence; paste example-alert.json in slides.
python3 demo.py query
python3 demo.py logs
python3 demo.py dashboard
python3 demo.py publish
```

The agent stops at findings and dashboard handoff. `operator-fault.py` is a separate operator-only fixture control, not an MCP tool or remediation feature. `paymentFailure=100%` deterministically throws when the payment charge path is exercised; real traffic, pipeline delivery, status metrics and warning logs must still be verified.


### Mac / Docker Desktop handoff

Use a separate clone of branch `demo/interview-walkthrough`. Install no cloud credentials. The full desktop browser needs **linux/amd64**; on Apple Silicon enable Docker Desktop amd64 emulation and expect a slower source build. Generated services explicitly select amd64. Reserve at least 14 GiB runtime disk **plus Rust/Nix build headroom** (30+ GiB free recommended), and allocate at least 8–12 GiB RAM to Docker. Inspect the Mac resources before starting.

```sh
cd demos/interview-walkthrough
./launch.sh prepare --binary /unused --skip-build
./build-portable.sh
./launch.sh up
./launch.sh serve
```

`--skip-build` only prepares configuration; `/unused` is never read. The portable script builds this checkout through `Dockerfile.source`, then adds Python and the demo object/policy fixture. It does not use the default Dockerfile’s latest release. Source compilation and original browser Nix build require working public build networking. This path is prepared but **not tested on a Mac**; the local task owns that validation. No Railway deployment is needed.

## Pi paired conversation/filesystem forks

Pi is pinned to `@earendil-works/pi-coding-agent@1.4.0`. The custom extension registers native MCP per session, cancels unpaired native forks, reserves an FS base, commits a binding to the newly forked Pi session, and tracks cancelled/uncertain states. It does not merge conversation histories. Unbound sessions and `fs`/session overrides fail closed.

Use a separately approved model setup; none is bundled or inferred from host credentials. Install the pinned package with `npm ci --ignore-scripts` in `pi/`, or explicitly run the optional Compose `agent` profile after model setup. Default container command is `--help`; startup does not make paid calls. Set `MCP_JS_DEMO_TOKEN` to the ephemeral token in your own `.state/ownership.json` without pasting it into shared terminals/logs. Then start Pi with `--tools codemode --extension ./paired-fork.ts` from `pi/`. Use `/demo-init`, add a synthetic investigation message, `/paired-fork`, and `/paired-merge OTHER_SESSION_ID`. Check `branch_info` before every write. The two branch roots must share an explicit base.

Run the no-model test while the controller and query worker are running:

```sh
MCP_JS_DEMO_STATE=/absolute/demo/.state node pi/verify-pairing.mjs
```

This tests actual Pi session files and MCP transport; it does not substitute for interactive extension hook testing. See `pi/agent-prompt.md` for the bounded investigation prompt.

## Publication boundary and supporting tests

The virtual COW tree is not a host directory. `observability.py` explicitly reads one selected COW root, validates the fixed dashboard UID, local datasource and timeseries panel types, writes/fsyncs a temporary file in a scoped shared volume, and renames it. Grafana polls that directory. Branches cannot directly modify the live Grafana volume. After paired merge, select that merged root as `dashboard_root` through the operator selection command before publishing; do not publish an unmerged branch by accident.

```sh
python3 select-root.py SESSION_ID
python3 demo.py publish
python3 verify-browser.py
python3 demo.py test   # own three nodes: includes controlled stop/restart + quorum loss
```

Browser evidence must show nonempty time-series graphs, not just API success. `verify-browser.py` records actual tool output and screenshot for human inspection; it does not mark canvas count as proof of data.

Cleanup only owned resources: `./launch.sh down` keeps volumes/journals; `./launch.sh reset` deletes owned volumes too, preserving the journal. Use a new state directory after reset. Never run blanket prune. Test data, blobs, labels and partial fork journals remain available for recovery inspection until explicit reset.

See [source-map.md](source-map.md), [architecture-reference.md](architecture-reference.md), and [THIRD_PARTY.md](THIRD_PARTY.md).

## Narrated companion

The five-chapter [Manim/Phonon source and player](animation/README.md) fit inside the 45-minute agenda. Use the Narrated companion button. Each chapter waits for presenter resume at live-demo checkpoints. Cloud rendering is blocked by Pango development dependencies, image-extraction disk space, and model-download HTTP 403; no narrated media is claimed complete.
