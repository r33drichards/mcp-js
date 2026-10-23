# Minimal LGTM + mcp-js + Pi

On the prepared Ubuntu instance:

```sh
cd /opt/mcp-interview/lgtm-pi-minimal/demos/lgtm-pi
./agent.sh
```

That is the agent startup command. It discovers the installed Node runtime, loads Pi 1.4.0 and connects `dashboards` and `scratch` MCP namespaces. It defaults to the previously approved Bedrock Nova Micro profile in us-west-2 using the instance role. No credentials are embedded. No model call is made until you send a prompt. Interactive requests are billed; the existing instance shutdown and IAM expiry still apply. This launcher does not implement an account-wide dollar cap.

From your Mac, open the Grafana tunnel:

```sh
ssh -i ~/.ssh/id_ed25519 -L 39300:127.0.0.1:39300 ubuntu@54.200.10.22
```

Open http://127.0.0.1:39300. No login is needed for the demo Viewer.

Example prompt: “Inspect the checkout metrics and logs. Create a dashboard showing the latency anomaly, then open it in the browser tool and verify the plotted data.”

## Start, seed and reset

```sh
./demo.sh status
./demo.sh up
./demo.sh seed
./demo.sh agent
```

The stack is already up and seeded on this instance. `seed` sends exactly two OTLP requests containing 61 metric points and 61 correlated log entries over ten minutes; eleven samples show a synthetic 900ms payment-timeout anomaly. Timestamps are fresh, values deterministic. Run it again if the sample window gets old.

`./demo.sh reset` archives generated dashboard JSON under `.evidence/`, then reseeds. It preserves historical telemetry, COW sessions and all Docker volumes. Start a fresh Pi conversation for fresh scratch. Stop any active agent before resetting.

## Exactly two Compose services

`compose.yaml` defines only `lgtm` and `mcp-js`. The official Grafana image `grafana/otel-lgtm` bundles the collector, Prometheus, Loki, Tempo and Grafana. The mcp-js container runs the two small engine endpoints plus the official Grafana MCP and computeruse.site browser processes. There is no publisher, Astronomy Shop, S3 fixture or separate browser service.

- `./dashboards` is mounted read-write at `/dashboards` in mcp-js and read-only at `/dashboards` in LGTM. The host-filesystem engine has no COW overlay. `fs.writeFile('/dashboards/checkout.json', JSON.stringify(dashboard))` immediately changes the shared file; Grafana's provisioner scans it every two seconds. No publish command or dashboard-write API is involved.
- Filesystem policy permits only JSON files directly in `/dashboards`. Other host paths are denied. Grafana MCP is configured read-only.
- The separate `scratch` endpoint stores COW snapshots under a named Docker volume. Its virtual namespace is `/work/`; it cannot write dashboards. Forking a COW root does not fork, undo or merge external dashboard effects. This minimal extension does not automatically pair Pi conversation forks with filesystem forks.
- Host ports are loopback-only: Grafana39300, OTLP39418, dashboard MCP39301, scratch MCP39302. Browser connects internally to `http://lgtm:3000`.

## Reproducibility and verification

LGTM and Grafana MCP images are pinned by digest. Pi dependencies are lockfile-pinned. `runtime-images.json` records exact IDs and source commits of the two locally built engine/browser base images. Those images are already available on the prepared Ubuntu host under independent minimal-demo tags. `docker compose up --build` rebuilds the small combined image. A fresh unrelated machine must first build or import those recorded base images; this is not claimed as a clean-machine bootstrap. No old running services or old demo files are required.

`python3 verify.py` performs real Grafana MCP metric/log queries, direct filesystem write, provisioning readback, denied-path check and browser screenshot. It creates a test dashboard; archive it with `./demo.sh reset` afterward. `node verify-pi.mjs` loads the real Pi extension and tests both registered MCP connections without a model. The inspected screenshot showed the 900ms spike and 80–100ms baseline. COW isolation and denial of scratch-to-dashboard writes were separately verified. No paid model E2E was run for this minimal demo.

Official image source: https://github.com/grafana/docker-otel-lgtm . Browser source: https://github.com/r33drichards/computeruse.site .
