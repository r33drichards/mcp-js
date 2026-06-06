# Clustering & replication (Raft)

In this tutorial we'll start a three-node mcp-v8 cluster with Docker Compose, verify leader election, and send a request through the nginx load balancer.

## Prerequisites

- Docker with Compose v2 (`docker compose version`)
- `curl` and `jq` on your path
- The mcp-v8 repository checked out locally

## Step 1 — Start the cluster

The repository ships a ready-made three-node cluster definition:

```bash
docker compose -f docker-compose.cluster.yml up --build
```

Docker builds the server image and starts four containers:

| Container | Role | Published port |
|-----------|------|----------------|
| `node1`   | MCP node (port 3000) + Raft peer (port 4000) | — |
| `node2`   | MCP node (port 3000) + Raft peer (port 4000) | — |
| `node3`   | MCP node (port 3000) + Raft peer (port 4000) | — |
| `lb`      | nginx load balancer | **8080 → `/mcp`** |

The three cluster nodes talk to each other on port 4000. The load balancer exposes port 8080 to the host and distributes `/mcp` requests across all three nodes.

Wait for lines similar to the following in the logs before proceeding:

```
node1  | [node1] Cluster node started on port 4000
node1  | [node1] Won election for term 1 (2/3 votes)
```

One node wins the election and becomes the Raft leader. The others become followers.

## Step 2 — Verify cluster health

The nginx config maps `/health` to the `/raft/status` endpoint on any backend node. Poll it to confirm the cluster is up:

```bash
curl -s http://localhost:8080/health | jq .
```

Expected output (leader node shown):

```json
{
  "node_id": "node1",
  "role": "Leader",
  "term": 1,
  "leader_id": "node1",
  "commit_index": 0,
  "last_applied": 0,
  "log_length": 0,
  "peers": ["node2:4000", "node3:4000"],
  "peer_addrs": {
    "node1": "node1:4000",
    "node2": "node2:4000",
    "node3": "node3:4000"
  }
}
```

`"role": "Leader"` confirms that a leader has been elected. If `role` is `"Follower"` or `"Candidate"`, wait a few seconds and retry — election can take up to the configured `--election-timeout-max` (2 000 ms in this example).

## Step 3 — Send a request through the load balancer

The nginx config proxies `POST /mcp` to the cluster via round-robin. Send an MCP `initialize` request to start a session:

```bash
curl -s -X POST http://localhost:8080/mcp \
  -H 'Content-Type: application/json' \
  -d '{
    "jsonrpc": "2.0",
    "id": 1,
    "method": "initialize",
    "params": {
      "protocolVersion": "2024-11-05",
      "capabilities": {},
      "clientInfo": {"name": "tutorial", "version": "1.0"}
    }
  }' | jq .result.serverInfo
```

The server returns its identity. Each response may come from a different node — that is the load balancer routing at work. Because mcp-v8 replicates the session log over Raft, session metadata is consistent across all nodes regardless of which one handles the request.

## Step 4 — Observe Raft replication in action

Query the Raft status of a specific node by exec-ing into its container:

```bash
docker compose -f docker-compose.cluster.yml exec node2 \
  curl -s http://localhost:4000/raft/status | jq '{role, leader_id, commit_index}'
```

The follower reports the same `commit_index` as the leader once replication catches up.

## Step 5 — Stop the cluster

```bash
docker compose -f docker-compose.cluster.yml down -v
```

The `-v` flag removes the named volumes so the next run starts with a clean state.

## What we covered

- The `docker-compose.cluster.yml` file defines a three-node cluster plus an nginx load balancer.
- Each node runs on two ports: an MCP HTTP port (3000) and a Raft cluster port (4000).
- Cluster mode requires `--http-port` or `--sse-port`; stdio transport is not supported.
- `/health` on the load balancer proxies to `/raft/status` on a backend node.
- Session-log metadata is replicated via Raft; V8 heap snapshots are stored separately in each node's heap storage backend.

## See also

- [Concepts: Clustering & replication (Raft)](../concepts/clustering.md)
- [How-to: Clustering & replication (Raft)](../how-to/clustering.md)
- [Reference: Clustering & replication (Raft)](../reference/clustering.md)
- [Concepts: Heap storage backends](../concepts/storage-backends.md)
- [Concepts: Transports](../concepts/transports.md)
- [Concepts: Stateful sessions & heap snapshots](../concepts/sessions-and-heaps.md)
- [Reference: CLI flags](../reference/cli-flags.md)
