# Docker Reference

## Dockerfile

| Property | Value |
|----------|-------|
| Build stage base | `rust:latest` |
| Runtime base | `debian:trixie-slim` |
| User | `mcpuser` (UID 1000) |
| Binary path | `/usr/local/bin/mcp-v8` |
| Data directory | `/data` (owned by `mcpuser`) |
| Exposed port | `8080` |
| Entrypoint | `mcp-v8` |
| Default CMD | `--sse-port 8080 --stateless` |

### Runtime Dependencies

- `ca-certificates`
- `libssl3`

### Overriding Arguments

The Dockerfile uses `ENTRYPOINT` for the binary and `CMD` for default arguments. Override just the arguments:

```bash
# Stateless stdio mode
docker run <image> --stateless

# Streamable HTTP on port 8080
docker run <image> --http-port 8080 --stateless

# Stateful SSE mode
docker run -p 8080:8080 <image> --sse-port 8080
```

## Docker Compose Files

| File | Description |
|------|-------------|
| `docker-compose.yml` | Default compose file |
| `docker-compose.single-node.yml` | Single node deployment |
| `docker-compose.single-node-stateful.yml` | Single node with stateful heap persistence |
| `docker-compose.cluster.yml` | Multi-node Raft cluster |
| `docker-compose.cluster-stateless.yml` | Multi-node stateless cluster |
| `docker-compose.secure-sessions.yml` | Deployment with secure session configuration |
| `docker-compose.module-policy.yml` | Deployment with module import policies |

## Environment Variables in Docker

Standard environment variables can be passed to the container:

```yaml
environment:
  - AWS_ACCESS_KEY_ID=...
  - AWS_SECRET_ACCESS_KEY=...
  - AWS_DEFAULT_REGION=us-east-1
  - JWKS_URL=https://auth.example.com/.well-known/jwks.json
```

## Volumes

For stateful mode, mount a volume to persist data:

```bash
docker run -v mcp-data:/data <image> --sse-port 8080 --directory-path /data/heaps --session-db-path /data/sessions
```
