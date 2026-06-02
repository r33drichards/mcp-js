# How to Deploy with Docker and docker-compose

Run mcp-v8 in containers using the provided compose files.

## Single-node deployment

```bash
docker compose -f docker-compose.single-node.yml up --build
```

This starts a single mcp-v8 node in stateless mode behind an nginx reverse proxy. The server is accessible at `http://localhost:8080/mcp`.

## Single-node stateful deployment

```bash
docker compose -f docker-compose.single-node-stateful.yml up --build
```

Runs with heap persistence using a local volume.

## 3-node cluster

```bash
docker compose -f docker-compose.cluster.yml up --build
```

This starts:

- 3 mcp-v8 nodes with Raft clustering
- nginx load balancer on port 8080
- Persistent volumes for heap and session data

Access at `http://localhost:8080/mcp`.

Each node runs with:

- `--http-port 3000`
- `--directory-path /data/heaps`
- `--session-db-path /data/sessions`
- `--cluster-port 4000`
- Raft timing: 200ms heartbeat, 1000-2000ms election timeout

## 3-node stateless cluster

For maximum throughput without heap persistence:

```bash
docker compose -f docker-compose.cluster-stateless.yml up --build
```

Same cluster setup but with `--stateless` -- no volumes needed.

## Build the Docker image

```bash
docker build -t mcp-v8 .
```

## Run a single container

```bash
docker run -p 3000:3000 mcp-v8 --http-port 3000 --stateless
```

With heap storage:

```bash
docker run -p 3000:3000 -v mcp-data:/data mcp-v8 \
  --http-port 3000 --directory-path /data/heaps --session-db-path /data/sessions
```

## Custom compose configuration

Use the provided compose files as templates. Key flags to configure:

| Flag | Purpose |
|------|---------|
| `--http-port` | HTTP listen port (required for network mode) |
| `--directory-path` | Heap storage path (mount a volume) |
| `--session-db-path` | Session DB path (mount a volume) |
| `--stateless` | Disable heap persistence |
| `--cluster-port` | Enable Raft clustering |
| `--node-id` | Unique node name |
| `--peers` | Comma-separated peer list (`id@host:port`) |

## Module and policy deployment

The `docker-compose.module-policy.yml` file demonstrates running with external module policies:

```bash
docker compose -f docker-compose.module-policy.yml up --build
```

## Secure sessions

The `docker-compose.secure-sessions.yml` file shows JWT-authenticated sessions:

```bash
docker compose -f docker-compose.secure-sessions.yml up --build
```
