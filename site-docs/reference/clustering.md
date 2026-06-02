# Clustering Reference

mcp-v8 supports Raft-based clustering for replicated state across multiple nodes.

## CLI Flags

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--cluster-port` | `u16` | (none) | Port for Raft cluster HTTP server. Enables cluster mode. |
| `--node-id` | `string` | `"node1"` | Unique node identifier within the cluster |
| `--peers` | `string` (comma-separated) | (none) | Seed peer addresses |
| `--join` | `string` | (none) | Join an existing cluster via this seed address (host:port) |
| `--advertise-addr` | `string` | `<node-id>:<cluster-port>` | Externally-reachable address for this node |
| `--heartbeat-interval` | `u64` | `100` | Heartbeat interval in milliseconds |
| `--election-timeout-min` | `u64` | `300` | Minimum election timeout in milliseconds |
| `--election-timeout-max` | `u64` | `500` | Maximum election timeout in milliseconds |

Cluster mode requires `--http-port` or `--sse-port` (stdio transport is not supported).

## Peer Format

The `--peers` flag accepts comma-separated peer addresses in two formats:

| Format | Example | Description |
|--------|---------|-------------|
| `host:port` | `10.0.0.2:4000` | Address only |
| `id@host:port` | `node2@10.0.0.2:4000` | Node ID with address |

## Raft HTTP Endpoints

### POST /raft/join

Register a new node with the cluster.

**Request body:**

```json
{
  "node_id": "node3",
  "addr": "10.0.0.3:4000"
}
```

### GET /raft/status

Get cluster status for this node.

**Response:**

```json
{
  "node_id": "node1",
  "role": "Leader",
  "term": 5,
  "leader_id": "node1",
  "commit_index": 42,
  "last_applied": 42,
  "log_length": 43,
  "peers": ["node2", "node3"],
  "peer_addrs": {
    "node2": "10.0.0.2:4000",
    "node3": "10.0.0.3:4000"
  }
}
```

## Raft Roles

| Role | Description |
|------|-------------|
| `Leader` | Handles all writes, sends heartbeats to followers |
| `Follower` | Receives and applies log entries from leader |
| `Candidate` | Running for leader election |

## Timing Defaults

| Parameter | Default | Description |
|-----------|---------|-------------|
| Heartbeat interval | 100 ms | Leader sends heartbeats at this interval |
| Election timeout min | 300 ms | Minimum wait before starting election |
| Election timeout max | 500 ms | Maximum wait before starting election |

The actual election timeout is randomized between min and max for each election cycle.

## Cluster Database

Each node stores its Raft log and replicated state in a sled database at:

```
{session-db-path}/cluster-{node-id}/
```

## Replicated State

The cluster replicates:

| Data | Storage Key Prefix | Description |
|------|--------------------|-------------|
| Heap tags | `ht:` | Tag key-value pairs per heap hash |
| Session log entries | `session:` | Session log entries |

Heap snapshots themselves are not replicated through Raft -- use S3 or shared storage for heap snapshot replication.

## Write Forwarding

Non-leader nodes forward write operations to the current leader. The `put_or_forward` method:

1. If this node is the leader, apply the write locally
2. If this node is a follower, forward the write request to the known leader via HTTP
3. The leader applies the write and replicates it through the Raft log

## Log Entry Format

```json
{
  "term": 5,
  "index": 42,
  "key": "ht:a3f2b8c1...",
  "value": "{\"env\":\"prod\"}"
}
```
