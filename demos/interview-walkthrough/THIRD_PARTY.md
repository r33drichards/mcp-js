# Attribution

The demo downloads pinned public sources during preparation rather than copying their entire trees. Preserve their license/notice files in the downloaded checkout and built images.

- OpenTelemetry Astronomy Shop 2.1.3 / 91356a39d5a0dc601317b55339b7abfafd91b481: OpenTelemetry Authors, Apache-2.0. Generated Compose and collector configuration derive from that checkout.
- r33drichards/computer-use / 63bab526d2a999af969657f70a342dfebe1cbb63: Apache-2.0; retain its LICENSE and THIRD_PARTY notices, including browser/desktop dependencies. No claim of production gVisor isolation in this Docker demo.
- Official grafana/mcp-grafana: see upstream license and image notices; pinned image digest in compose.yml.
- Pi @earendil-works/pi-coding-agent 1.4.0 and MCP SDK 1.31.0: preserve installed package licenses; package-lock.json pins the JavaScript dependency graph.
- Optional historical NAB CPU fixture: MIT Numenta; full notice in fixtures/NAB-LICENSE.txt. It is not the main Astronomy scenario. Source provenance and SHA-256 are in fixtures/provenance.json. Timestamps lack timezone offsets; benchmark annotations are not production root-cause proof.
