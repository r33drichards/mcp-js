# Docker Guide for mcp-v8

This guide explains how to build and run the mcp-v8 server using Docker.

## Building the Docker Image

Build the image from the project root:

```bash
docker build -t mcp-v8:latest .
```

The build process:
1. Uses Rust nightly toolchain (as required by the project)
2. Installs dependencies needed for V8 compilation
3. Builds the release binary
4. Creates a minimal runtime image with only necessary dependencies
5. Runs as non-root user for security

## Running the Container

### Streamable HTTP Server (Default)

```bash
docker run -p 8080:8080 mcp-v8:latest
```

This starts the Streamable HTTP server on port 8080. The MCP endpoint is served
at `/mcp` and a plain API at `/api/exec`. The default is stateless — no heap or
filesystem persistence, so each execution starts with a fresh V8 isolate.

The image's `ENTRYPOINT` is the `mcp-v8` binary, so arguments after the image
name are passed straight to it — do not repeat the binary name.

### Custom Port

Either set `$PORT` or pass the flag explicitly:

```bash
docker run -p 3000:3000 -e PORT=3000 mcp-v8:latest
docker run -p 3000:3000 mcp-v8:latest --http-port 3000
```

### Deploying to a Hosted Platform

Platforms such as Railway, Render, Heroku, Fly and Cloud Run assign a port at
runtime and inject it as `$PORT`, then route their health checks to it. `mcp-v8`
folds `$PORT` into `--http-port`, so the image deploys unmodified — no start
command or argument overrides:

```bash
PORT=52341 mcp-v8   # equivalent to: mcp-v8 --http-port 52341
```

Precedence, highest first:

1. An explicit `--http-port`/`--sse-port` argument.
2. `MCP_V8_HTTP_PORT` / `MCP_V8_SSE_PORT`.
3. `http_port` / `sse_port` in a `--config` file.
4. `$PORT` (always Streamable HTTP).
5. No port at all — the stdio transport.

Because the container sets `PORT=8080` by default, use `-e PORT=` to clear it
and get stdio:

```bash
docker run -i -e PORT= mcp-v8:latest
```

`--metadata-only` cluster nodes serve no MCP transport and ignore `$PORT`
entirely rather than failing at startup.

The container binds `0.0.0.0`, so it accepts any `Host` header by default and
works behind a platform domain or reverse proxy with no extra configuration. To
restrict it to your own hostname, pass `--allowed-hosts`:

```bash
docker run -p 8080:8080 mcp-v8:latest --allowed-hosts mcp.example.com
```

See [Control `Host` and `Origin` validation](site-docs/how-to/transports.md) for
the full rules.

### With Persistent Storage (Volume)

```bash
docker run -p 8080:8080 -v mcp-v8-data:/data mcp-v8:latest \
  --http-port 8080 \
  --heap-store dir --heap-dir /data/heaps \
  --session-db-path /data/sessions
```

Persistence is opt-in: `--heap-store dir` turns on heap snapshots, and the named
volume keeps them across container restarts. Use `--fs-store dir --fs-dir
/data/fs` to persist the virtual filesystem the same way.

Point `--session-db-path` at the volume too. It holds the session log (the
per-session heap+fs history) and the execution registry, and is the default
parent for the heap-tag store, fs blob store, and fs label database. It defaults
to `/tmp/mcp-v8-sessions`, so leaving it off the volume means the heap snapshots
survive a restart while the session history and tags pointing at them do not.

Note that heap persistence is mutually exclusive with WebAssembly: heap
snapshots require a V8 `SnapshotCreator` isolate, which disables WASM, so
combining `--heap-store` with `--wasm-module`/`--wasm-config` is rejected at
startup.

### With S3 Storage

```bash
docker run -p 8080:8080 \
  -e AWS_ACCESS_KEY_ID=your_access_key \
  -e AWS_SECRET_ACCESS_KEY=your_secret_key \
  -e AWS_REGION=us-east-1 \
  mcp-v8:latest \
  --http-port 8080 --heap-store s3 --s3-bucket your-bucket-name
```

### With S3 and Write-Through Cache

```bash
docker run -p 8080:8080 \
  -e AWS_ACCESS_KEY_ID=your_access_key \
  -e AWS_SECRET_ACCESS_KEY=your_secret_key \
  -e AWS_REGION=us-east-1 \
  mcp-v8:latest \
  --http-port 8080 --heap-store s3 --s3-bucket your-bucket-name --cache-dir /tmp/mcp-v8-cache
```

### SSE Transport

```bash
docker run -p 8081:8081 mcp-v8:latest --sse-port 8081
```

Exposes `/sse` for the event stream and `/message` for client requests. This is
the legacy transport and does **not** serve `/mcp`; most clients and hosted
deployment health checks expect Streamable HTTP, so prefer the default
`--http-port`.

## Docker Compose

Create a `docker-compose.yml` file:

```yaml
version: '3.8'

services:
  mcp-v8:
    build: .
    command:
      - --http-port=8080
      - --heap-store=dir
      - --heap-dir=/data/heaps
      - --session-db-path=/data/sessions
    ports:
      - "8080:8080"
    volumes:
      - mcp-v8-data:/data
    environment:
      - RUST_LOG=info
    restart: unless-stopped

volumes:
  mcp-v8-data:
```

Run with:

```bash
docker-compose up -d
```

See also the pre-configured compose files in the repository root:
- `docker-compose.single-node.yml` — Single node behind nginx
- `docker-compose.single-node-stateful.yml` — Single node, stateful
- `docker-compose.cluster.yml` — 3-node Raft cluster behind nginx
- `docker-compose.cluster-stateless.yml` — 3-node Raft cluster, stateless

## Testing the Server

Once running, test the connection:

```bash
# Test the MCP endpoint with an initialize handshake (Streamable HTTP).
# A bare `curl http://localhost:8080/mcp` returns 406 — the endpoint requires
# a POST plus the JSON and SSE Accept types.
curl -X POST http://localhost:8080/mcp \
  -H "Content-Type: application/json" \
  -H "Accept: application/json, text/event-stream" \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"curl","version":"1"}}}'

# Test the plain HTTP API directly. This returns an execution_id; fetch the
# result with GET /api/executions/<id>/output.
curl -X POST http://localhost:8080/api/exec \
  -H "Content-Type: application/json" \
  -d '{"code": "console.log(1 + 2)"}'

# Use MCP Inspector
npx @modelcontextprotocol/inspector http://localhost:8080/mcp
```

## Environment Variables

- `AWS_ACCESS_KEY_ID` - AWS access key for S3 storage
- `AWS_SECRET_ACCESS_KEY` - AWS secret key for S3 storage
- `AWS_REGION` - AWS region for S3 bucket
- `RUST_LOG` - Logging level (debug, info, warn, error)

## Security Notes

- The container runs as non-root user (mcpuser, UID 1000)
- Only essential runtime dependencies are included
- The HTTP server binds to 0.0.0.0 to accept connections from outside the container
- Consider using secrets management for AWS credentials in production
- In production, use a reverse proxy (nginx, traefik) for TLS termination

## Troubleshooting

### Container Exits Immediately

Check logs:
```bash
docker logs <container-id>
```

### Port Already in Use

Change the host port:
```bash
docker run -p 9090:8080 mcp-v8:latest
```

### S3 Access Issues

Verify AWS credentials and permissions:
```bash
docker run -it --entrypoint bash mcp-v8:latest -c "env | grep AWS"
```

## Building for Different Architectures

Build for ARM64 (Apple Silicon, ARM servers):
```bash
docker buildx build --platform linux/arm64 -t mcp-v8:arm64 .
```

Build for AMD64 (most servers):
```bash
docker buildx build --platform linux/amd64 -t mcp-v8:amd64 .
```

Multi-platform build:
```bash
docker buildx build --platform linux/amd64,linux/arm64 -t mcp-v8:latest .
```
