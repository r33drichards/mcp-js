# Clustering & replication (Raft)

Complete reference for all cluster-related CLI flags, the peer address format, Raft HTTP endpoints, and the cluster database layout.

## CLI flags

All timeout values are in **milliseconds**.

| Flag | Default | Description |
|------|---------|-------------|
| `--cluster-port <PORT>` | _(none)_ | Port for the Raft cluster HTTP server. Setting this flag enables cluster mode. Requires `--http-port` or `--sse-port` to also be set. |
| `--node-id <ID>` | `node1` | Unique string identifier for this node within the cluster. Used as the node's name in log entries, peer tables, and status responses. |
| `--peers <LIST>` | _(empty)_ | Comma-separated list of peer addresses. Each entry is in `id@host:port` format. The node does not include itself. |
| `--join <host:port>` | _(none)_ | Address of any existing cluster member to contact at startup to join the cluster. Sends `POST /raft/join` to the seed and the request is forwarded to the leader. |
| `--advertise-addr <host:port>` | `{node-id}:{cluster-port}` | The cluster address this node advertises to peers. Override when the default derived name is not resolvable (e.g. on bare-metal with IP addresses). |
| `--heartbeat-interval <MS>` | `100` | How often the leader sends `AppendEntries` heartbeats to followers (milliseconds). |
| `--election-timeout-min <MS>` | `300` | Lower bound of the randomised election timeout window (milliseconds). |
| `--election-timeout-max <MS>` | `500` | Upper bound of the randomised election timeout window (milliseconds). |

## Peer address format

```
id@host:port
```

- `id` — the `--node-id` of the peer node.
- `host` — DNS name or IP address of the peer's machine.
- `port` — the peer's `--cluster-port`.

Multiple entries are comma-separated on a single `--peers` flag:

```
--peers=node2@node2:4000,node3@node3:4000
```

The `id@` prefix is optional. If omitted the address is stored without a named ID:

```
--peers=node2:4000,node3:4000
```

Named entries are preferred — they enable the cluster status response to map node IDs to addresses.

## Raft HTTP endpoints

These endpoints are served on the `--cluster-port`, not the MCP transport port. They are for inter-node Raft traffic and cluster administration.

### `GET /raft/status`

Returns the current Raft state of the node.

**Response** `200 application/json`:

```json
{
  "node_id": "node1",
  "role": "Leader",
  "term": 3,
  "leader_id": "node1",
  "commit_index": 42,
  "last_applied": 42,
  "log_length": 42,
  "peers": ["node2:4000", "node3:4000"],
  "peer_addrs": {
    "node1": "node1:4000",
    "node2": "node2:4000",
    "node3": "node3:4000"
  }
}
```

`role` is one of `"Leader"`, `"Follower"`, or `"Candidate"`.

### `POST /raft/join`

Registers a new peer with the cluster. If the contacted node is not the leader, it forwards the request to the current leader.

**Request body** `application/json`:

```json
{
  "node_id": "node4",
  "addr": "node4:4000"
}
```

| Field | Type | Description |
|-------|------|-------------|
| `node_id` | string | The `--node-id` of the joining node. |
| `addr` | string | The `host:port` the joining node advertises (its `--advertise-addr` or `--cluster-port` derivation). |

**Response** `200` on success:

```json
{ "ok": true }
```

**Response** `503` if no leader has been elected yet or the join could not be forwarded.

### `POST /raft/leave`

Removes a peer from the cluster. Must be sent to a node that can reach the current leader.

**Request body** `application/json`:

```json
{ "node_id": "node4" }
```

**Response** `200 { "ok": true }` on success; `503` if no leader or node not found.

### `POST /raft/append-entries`

Internal Raft RPC used by the leader to replicate log entries and send heartbeats to followers. Not intended for external callers.

### `POST /raft/request-vote`

Internal Raft RPC used by candidates to solicit votes during leader election. Not intended for external callers.

## Cluster database layout

Each node stores Raft state in a sled database at:

```
{session-db-path}/cluster-{node-id}
```

With the default `--session-db-path=/tmp/mcp-v8-sessions`, node1's cluster DB is at `/tmp/mcp-v8-sessions/cluster-node1`.

| sled key / tree | Contents |
|-----------------|---------|
| `raft_meta` | JSON object with `current_term` (u64) and `voted_for` (string or null). |
| tree `raft_log` | Ordered log entries, each a `LogEntry` with fields `term`, `index`, `key`, `value`. |
| `raft_peers` | JSON map of `node_id → host:port` for all known peers. |
| tree `data` | Applied key-value entries (the committed state machine). |

The cluster DB is separate from the session sled DB used by the MCP service. The session log and heap tag stores write their changes into the Raft log and are applied to tree `data` on commit.

## Startup constraint

Cluster mode (`--cluster-port` set) requires exactly one of `--http-port` or `--sse-port`. Providing `--cluster-port` without an HTTP or SSE transport causes the server to exit immediately with:

```
Cluster mode requires --http-port or --sse-port (stdio transport is not supported in cluster mode)
```

## See also

- [Tutorial: Clustering & replication (Raft)](../tutorials/clustering.md)
- [How-to: Clustering & replication (Raft)](../how-to/clustering.md)
- [Concepts: Clustering & replication (Raft)](../concepts/clustering.md)
- [Concepts: Heap storage backends](../concepts/storage-backends.md)
- [Concepts: Transports](../concepts/transports.md)
- [Concepts: Stateful sessions & heap snapshots](../concepts/sessions-and-heaps.md)
- [Reference: CLI flags](cli-flags.md)
