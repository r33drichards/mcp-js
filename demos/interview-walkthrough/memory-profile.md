# Staged profile for the 7.65 GiB Docker VM

This is an explicit two-phase topology, not a smaller simultaneous HA deployment. It retains the real Astronomy Shop checkout/payment path, application OTLP metrics/logs, Grafana MCP, browser, Pi binding and COW filesystem investigation. It stages the independent three-voter failure demonstration afterward. **Configuration validation passed; this workload's runtime memory and end-to-end behavior have not yet been measured.**

## Why these services can be removed

Pinned Astronomy 2.1.3 provides [`docker-compose.minimal.yml`](https://github.com/open-telemetry/opentelemetry-demo/blob/91356a39d5a0dc601317b55339b7abfafd91b481/docker-compose.minimal.yml). Its checkout omits `KAFKA_ADDR`; [`src/checkout/main.go`](https://github.com/open-telemetry/opentelemetry-demo/blob/91356a39d5a0dc601317b55339b7abfafd91b481/src/checkout/main.go) creates the Kafka client only when that address is nonempty. The staged profile follows that seam, removing Kafka/accounting/fraud. It preserves every direct checkout service and frontend dependency used by the selected path, with their original memory caps unchanged.

The original 1.5 GiB load generator includes browser-driven traffic and broad scenarios. We replace it with **one real HTTP cart-plus-checkout transaction every ten seconds**, capped at 180 attempts. Request shapes and the clearly synthetic card fixture match upstream `src/load-generator/locustfile.py` and `people.json`. The driver never writes Prometheus samples or fake logs; telemetry still comes from the actual services. It is not a throughput benchmark or a reproduction of upstream load patterns. Parent trace spans from the original generator are absent.

Jaeger trace archival and duplicate OpenSearch log indexing are omitted. Real spans still feed the Collector spanmetrics connector; real OTLP logs go to the already-declared Loki datasource. No claims about trace search or OpenSearch behavior apply in this phase. Host/database/Redis polling receivers are omitted; the collector accepts application OTLP only. The storefront is exposed directly on loopback 38880, so proxy-only feature/trace UI paths are not part of this mode. Operator file-based flag injection remains available.

The investigation runs one mcp-js voter that also hosts the query runtime. Its metadata uses a **new solo volume**, never a three-voter database with silently changed membership. It shares the browser loopback namespace, preserving upstream Host/Origin protection. Pi's branch mapping remains active. The controller maps logical query node 4 to node 1 only after explicit phase activation. The node-failure and quorum actions reject the single-node phase.

## Static budget, not a measured fit

Default investigation: 25 services, **6,649 MiB aggregate configured memory ceilings**. The original app, browser (2 GiB), and engine (768 MiB) ceilings are unchanged. Previously uncapped support services now have safety ceilings: Grafana 512 MiB, Loki 512 MiB, Grafana MCP 256 MiB, object fixture 128 MiB, publisher/traffic 64 MiB each and namespace keeper 32 MiB. These are explicit bounds to validate, not measured minimum requirements. Host-run Pi avoids another 512 MiB optional container; do not enable the `agent` profile within this budget. Static preflight reserves a further 512 MiB for Docker overhead, requiring 7,161 MiB total. A passing preflight does not establish acceptable swap pressure or actual peak memory.

Existing cloud component-only observations were roughly 32–57 MiB per voter, 43 MiB query worker, 89 MiB Grafana, 14 MiB Grafana MCP and 33 MiB Prometheus. They were idle/recent-run observations of the earlier small fixture, **not Astronomy/browser peaks** and not proof this profile fits.

The cluster phase contains only the original three voters plus objects: 2,432 MiB configured ceilings. It cannot run simultaneously with the investigation under this budget. Its successful failover would not prove availability of the stopped Astronomy/Grafana/browser workflow.

## Exact Mac handoff

Use the latest branch snapshot containing this file. Keep the six existing volumes and local Docker credentials/configuration. If Docker Desktop CLI plugins require `cliPluginsExtraDirs`, preserve the locally established setting; do not copy cloud credentials. Build before launching a phase, and avoid concurrent Rust/Nix builds and the workload.

From `demos/interview-walkthrough`, with the existing `MCP_JS_DEMO_STATE` pointing to the prepared `mcp-interview-781c97f6` state directory:

```sh
# Existing full preparation and portable image build are prerequisites.
# Do not rerun preparation blindly: it can overwrite an operator's flag fixture.
./build-portable.sh
python3 memory-profile.py prepare
python3 memory-profile.py check
python3 memory-profile.py activate investigation
python3 memory-profile.py pull investigation
# Start ONLY this generated phase, not launch.sh up / the full model.
docker compose -f "$MCP_JS_DEMO_STATE/compose.investigation.json" --profile browser up -d --build
python3 memory-profile.py measure
python3 demo.py seed
python3 demo.py policy
python3 demo.py query
python3 demo.py logs
./launch.sh serve
```

`pull investigation` resolves only this phase's tag-based external images to digests. It does not pull omitted heavyweight services. It does not pull/build the optional Pi image. The browser image remains the exact pinned computer-use desktop and may still be the expensive build; no different browser stack is substituted.

After health and the first 3–5 minutes, capture `memory-profile.py measure` again. Inspect `docker stats`, OOM flags, restart counts, Docker VM pressure and host swap. Stop the owned phase if it OOMs, restarts persistently, has no meaningful headroom, or creates sustained host memory pressure. Do not keep lowering caps to force a green launch. Verify nonempty logs and metrics before fault injection; verify a baseline successful checkout, then the `paymentFailure=100%` error path and actual payment warning logs. Neither the traffic driver's HTTP status nor a healthy container proves telemetry arrived.

Run Pi on the host using the documented isolated package/extension and an approved model setup. The model remains optional and no paid call is made by these commands. Perform policy, fork/merge, selected-root publication and actual graph/browser verification. Save evidence before changing phases.

For the separate cluster demonstration:

```sh
./launch.sh down  # only owned resources; keeps all volumes/journals
python3 memory-profile.py activate cluster
python3 memory-profile.py pull cluster
docker compose -f "$MCP_JS_DEMO_STATE/compose.cluster.json" up -d
python3 memory-profile.py measure
python3 demo.py test
```

Phase activation refuses while any owned container is running. It archives `demo.json`, `pairs.json`, alert/query/handoff files under `phase-state/<phase>` before selecting a phase's own state. Content/label volumes are never removed. Returning to investigation after `down` restores its archived roots/session mappings; do not reseed if resuming that investigation. Global events remain an append-only record. Use `down`, not `reset`, between phases.

Validation: both generated Compose models validate with Docker Compose; six focused tests cover dependency closure, actual OTLP pipeline routing, unchanged stock caps, bounded support caps, distinct solo storage and separate three-voter phase. No runtime was launched by this change. Exact measured workload fit and Mac emulation behavior remain the local task's next checks.
