# Minimal LGTM + mcp-js + Pi

On the prepared Ubuntu instance:

```sh
cd /opt/mcp-interview/lgtm-pi-minimal/demos/lgtm-pi
./agent.sh
```

That command discovers the installed Node runtime, loads Pi 1.4.0 and connects the `dashboards` and `scratch` MCP namespaces. The launcher does not force a provider or model.

Inside Pi:

1. Run `/login openai`.
2. Choose **Sign in with ChatGPT** (the installed provider is `openai`, not an assumed `openai-codex` alias).
3. Complete the browser sign-in yourself. If the remote SSH callback cannot reach Pi, paste the final redirect URL or authorization code into your own Pi terminal when it asks. Do not paste credentials into shared chat.
4. Run `/model` and select an OpenAI model shown as available for your subscription. Pi filters unsupported models after login; no guessed latest model ID is hard-coded here.
5. Send your dashboard investigation prompt.

We have not initiated OAuth or authenticated on your behalf. Login and model entitlement are still yours to complete. No credentials are embedded or exported. Saved model choice can persist between sessions; check `/model` before sending the first prompt.

Optional explicit Bedrock route, using the existing scoped instance role:

```sh
./agent.sh --provider amazon-bedrock --model us.amazon.nova-micro-v1:0
```

From the main repo use `./demos/lgtm-pi/demo.sh agent --provider amazon-bedrock --model us.amazon.nova-micro-v1:0`. This is optional and billed to AWS; Qwen is not granted. Existing instance shutdown and IAM expiry remain unchanged.

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

## Pi instructions and reload

Pi supports project `.pi/APPEND_SYSTEM.md` and `--append-system-prompt`. This demo uses the latter: `agent.sh` reads the complete `agent-prompt.md` on every launch and adds it to Pi's system instructions. It explicitly separates host cwd, shared dashboard files, and COW scratch; dashboard directory listing remains forbidden by policy.

After updating the instructions, exit Pi and restart with `./demos/lgtm-pi/demo.sh agent` from `/opt/mcp-interview/repo`. Start a fresh conversation if the old one still contains misleading path assumptions. No re-login is needed solely for this prompt change. The launcher reads the file at startup; an already-running session does not automatically acquire edited CLI prompt text. `node verify-instructions.mjs` tests the installed Pi resource loader without a model call.

## MCP-only agent tools

Following [CUSE's explicit tool allowlist](https://github.com/r33drichards/cuse/blob/main/src/channel-session.ts), this launcher uses Pi's supported `--no-builtin-tools`, disables automatic extension/skill/context discovery, explicitly loads the MCP extension and this demo extension, and allowlists only:

- `mcp__dashboards__run_js`
- `mcp__dashboards__get_artifact`
- `mcp__dashboards__list_artifacts`
- `mcp__scratch__run_js`

The MCP servers use `exposure: direct`, so these tools reach the model without Pi's local codemode tool. Grafana and browser tools are called inside dashboard `run_js`; the existing server-side policies still apply. The full Pi agent wire catalog was checked: no built-in read/write/edit/bash/powershell/find/grep/ls, codemode, or other aliases are exposed. This configures the model's tools; it does **not** OS-sandbox the Pi process, its extensions, or manual user commands.

Exit and restart Pi with `./demos/lgtm-pi/demo.sh agent` from the main repository to receive this configuration. Your current conversation and ChatGPT authentication are not modified. `PI_OFFLINE=1 node verify-tool-catalog.mjs` checks an isolated no-model session using empty test auth storage, without reading your auth file.
