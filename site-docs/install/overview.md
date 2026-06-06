# Install

`mcp-v8` ships as a single server binary (`mcp-v8`) plus an optional CLI client
(`mcp-v8-cli`). Pick whichever installation method fits your environment.

## Nix (recommended)

The repo is a flake. Run the server directly:

```bash
# Run the latest server
nix run github:r33drichards/mcp-js -- --help

# Or build it into ./result/bin/server
nix build github:r33drichards/mcp-js
./result/bin/server --http-port=8080 --stateless
```

For local development, enter the dev shell (provides the Rust toolchain and the
pinned V8 archive so `cargo` works without downloading V8):

```bash
git clone https://github.com/r33drichards/mcp-js
cd mcp-js
nix develop          # then: cargo run -p server -- --help
```

> Building from source compiles V8 via `deno_core`; the flake wires up the
> prefetched `RUSTY_V8_ARCHIVE` so the build stays offline-friendly.

## Prebuilt binary (install scripts)

The install scripts download the matching release binary from GitHub and drop it in
`/usr/local/bin`:

```bash
# Server
curl -fsSL https://raw.githubusercontent.com/r33drichards/mcp-js/main/install.sh | bash

# CLI client
curl -fsSL https://raw.githubusercontent.com/r33drichards/mcp-js/main/install-cli.sh | bash
```

Set `MCP_V8_VERSION` to pin a specific release. Supported platforms: Linux x86_64,
Linux arm64, and macOS Apple Silicon (arm64).

## Download from a running server

A running HTTP/SSE server serves CLI binaries that exactly match its own version:

```bash
curl http://localhost:8080/api/cli                 # JSON index of available platforms
curl -L http://localhost:8080/api/cli/linux-x86_64 -o mcp-v8-cli && chmod +x mcp-v8-cli
```

Platforms: `linux-x86_64`, `linux-aarch64`, `macos-aarch64`. (Dev/local builds may
not embed binaries; those endpoints then return 404.)

## Docker / Docker Compose

The repo includes ready-made Compose stacks for common topologies:

| File | Topology |
|------|----------|
| `docker-compose.yml` | Single node + OPA, fetch enabled |
| `docker-compose.single-node.yml` | Single stateless node behind nginx |
| `docker-compose.single-node-stateful.yml` | Single stateful node (FS heaps) |
| `docker-compose.cluster.yml` | 3-node Raft cluster + nginx load balancer |
| `docker-compose.cluster-stateless.yml` | 3-node stateless cluster (benchmarking) |
| `docker-compose.module-policy.yml` | External modules gated by an OPA policy |
| `docker-compose.secure-sessions.yml` | Keycloak (JWKS) + OPA + server |

```bash
docker compose -f docker-compose.single-node-stateful.yml up --build
# server reachable at http://localhost:8080/mcp
```

## Verify

```bash
mcp-v8 --help            # or: ./result/bin/server --help
mcp-v8 --print-openapi   # prints the REST OpenAPI spec
```

## Next steps

- [Tutorials](../tutorials/overview.md) — run your first code.
- [Transports](../concepts/transports.md) — choose stdio, HTTP, or SSE.
- [CLI flags reference](../reference/cli-flags.md) — every configuration flag.
