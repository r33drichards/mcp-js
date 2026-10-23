# EC2 rehearsal handoff — deployment owned by the separate local task

The user approved **USD 20 total for 24 hours**, then stopping the instance. This document does not create infrastructure or authorize extra credentials/security changes. The separate AWS task must verify price, account/region, access, instance identity, budget headroom and the shutdown deadline. Retained EBS disks still incur charges after stopping. No AWS action was performed by the cloud preparation task.

The fixed code snapshot is `0b0517702c8f4f1a72b0d34a702ee7848b2586ea` or a verified descendant on `demo/interview-walkthrough`. The inspected engine base remains `fe5b613db0afcc9cf20d7ca1ef659c914a0b5008`. An x86_64 host with 8 vCPU / 32 GiB RAM / 100 GB disk is a proposed target, not an observed successful deployment or price guarantee. Use the full topology first when resources permit; retain `memory-profile.md` as an explicitly staged alternative.

## Host preparation and reproducible build

Prerequisites: Linux amd64, Docker Engine/Compose v2, Git, Python 3.12+ with PyYAML, ordinary build-network access. Use the existing approved access method. Do not publish Docker API, MCP/Raft/blob/browser ports, or unauthenticated Grafana to the internet. All published demo ports remain loopback. Access slides/Grafana/storefront through an approved forwarding mechanism; security-group or credential changes remain separately gated.

```sh
git clone --branch demo/interview-walkthrough https://github.com/r33drichards/mcp-js.git
cd mcp-js
# Confirm the exact agreed commit; do not assume a moving branch is unchanged.
git rev-parse HEAD
cd demos/interview-walkthrough
export MCP_JS_DEMO_STATE="$PWD/.state"
./launch.sh prepare --binary /unused --skip-build
./build-portable.sh
./launch.sh up
./launch.sh serve
```

Preparation materializes and verifies pinned upstream checkouts. The engine build exports a specific Git commit to a clean temporary context; ignored/untracked state never reaches Docker, and tracked sensitive paths are filtered too. The runtime context contains only three selected files from that export. Preserve the operator's `DOCKER_CONFIG`; no credentials need copying from cloud or Mac.

Cargo parallelism is one job, but linker/compiler peak memory is **not bounded** by that setting. Do not run source/browser builds concurrently with the workload if memory pressure appears. The source Dockerfile still uses upstream moving Rust/base-image references; the Git input is pinned, but bit-for-bit toolchain reproducibility is not established. Record resulting image IDs/digests and build logs. Do not substitute the default Dockerfile's latest released engine.

A previously tested debug Linux amd64 server binary is available in the cloud scratch artifact `mcp-js-fe5b613-linux-amd64.gz` with committed checksums in `evidence/cloud-binary-manifest.json`. It contains no state or credentials. Transfer to EC2 has **not** been performed, and the artifact is not in Git or a public release. Use only a separately coordinated authorized transfer, verify both gzip and decompressed SHA-256, then check target ABI. Do not guess an artifact URL or silently replace it with another binary.

## Acceptance order

1. Confirm all owned services are ready and no OOM/restart loop occurs. Save `docker compose ... ps`, image digests, memory and disk observations. Current full-stack readiness is unverified.
2. Start traffic and wait at least three telemetry export intervals. Confirm a real successful checkout and nonempty official Grafana MCP metric **and log** responses. Empty successful queries are not evidence of coverage.
3. Seed COW state; prove allowed/denied writes. Operator injects `paymentFailure=100%` only after the baseline. Confirm actual checkout error metrics and payment warning logs over the same window. Agent performs investigation/dashboard only, no remediation.
4. Exercise actual Pi extension hooks with an existing approved model setup. AWS budget approval is not approval for new paid model calls. Earlier tests exercised Pi session/MCP components without model calls.
5. Fork paired sessions/filesystems, preserve independent findings, merge explicitly, select the merged root and publish. Verify actual plotted data and browser screenshot; record root, dashboard hash, query windows and uncertainty.
6. Run the supporting owned-node recovery experiment only with a healthy three-voter baseline and a restoration plan. Observe timeout ambiguity. Shared objects remain a single failure domain.
7. Save evidence before cleanup. The AWS owner stops the **exact rehearsal instance** by the approved 24-hour deadline and reports retained volume charges/disposition. `launch.sh down` only removes labeled demo containers/networks; it does not stop EC2 or end AWS billing.

## Narration after normal model access is available

Five actual Manim silent previews and the 45-minute presenter-controlled player are already in Git. They are not narrated media. Cloud model metadata resolution at `https://huggingface.co/api/models/gradium/phonon-31m-en` was rejected by the configured proxy during CONNECT with HTTP 403. No model weights or speech were generated.

Narration is no longer on the critical path. Use the live slides and notebook; silent previews remain optional. Do not spend rehearsal setup time downloading speech models or rendering chapters.
