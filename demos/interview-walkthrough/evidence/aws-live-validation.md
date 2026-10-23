# AWS live rehearsal validation

The 2026-10-23 rehearsal ran the full pinned Astronomy Shop 2.1.3 workload,
source-built mcp-js engine and upstream computeruse.site browser on an isolated
8-vCPU / 32-GiB Linux instance. See `aws-live-validation.json` for the compact
receipt and `aws-dashboard-reviewed.png` for the visually reviewed baseline.
The engine source was db83bfb8; notebook/runtime scripts were da648070.

Eight bounded synthetic checkouts returned HTTP 200. The operator's temporary
paymentFailure flag caused three HTTP 500 responses and corresponding payment
failure logs. After restoring the flag, checkout returned 200. Grafana MCP
returned both STATUS_CODE_ERROR and STATUS_CODE_UNSET checkout rate series.
Notebook query/log/dashboard/publish actions used actual MCP and COW storage;
Grafana's loaded panels were compared with the staged dashboard before the
browser MCP captured both graphs with fresh plotted data.

## Actual telemetry wiring

The collector's logs pipeline exports to **opensearch, debug, and otlphttp/loki**.
The added Loki OTLP endpoint is `http://loki:3100/otlp`. Grafana datasource
`demo-logs` is Loki at `http://loki:3100`; the tested path is mcp-js → official
Grafana MCP `query_loki_logs` → Grafana → Loki. Stock OpenSearch remains running,
but its query path was not the path verified by this rehearsal.
`demo-prometheus` points to `http://prometheus:9090`.

## Startup fixes reproduced in source

- Render the pinned upstream Compose with DEMO_VERSION=2.1.3. Its default
  `latest` images did not match the pinned configuration and broke startup.
  External tags are subsequently resolved by lock-images.py as before.
- Put the datasource file inside the existing provisioning-directory bind.
  A nested bind under that read-only parent failed during container creation.
- Run generated engine services as UID/GID 1000, matching the source image's
  data directory. Root with all capabilities dropped could not write it.
  The alternate scratch-image builder now creates /data and copies it with
  matching ownership; this alternate image was not rebuilt in this rehearsal.

Fresh preparation rendered all 19 upstream release images correctly and
verified engine UID and the non-overlapping provisioning mount. All 17 focused
Python tests passed. The existing live generated configuration was preserved;
no reseed or destructive preparation was run against it.

Pi 1.4.0 SessionManager/native MCP pairing, isolation, rejected root override,
cancel journal and explicit merge passed without a model. Filesystem merge,
conflict handling, leader loss and quorum loss passed. The quorum-timeout write
later committed after recovery, demonstrating uncertain outcomes correctly.
All three voters were restored. Raw receipts remain in the disposable
`.state/live-evidence` directory; ownership tokens and generated credentials are
intentionally excluded from this repository.

Bedrock model execution and interactive Pi extension dispatch remain untested.
The reviewed screenshot is the successful baseline, not the later fault graph.
