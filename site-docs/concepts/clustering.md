# Raft Clustering

mcp-v8 supports multi-node clustering for high availability and data replication. The cluster uses a Raft consensus protocol to ensure that session logs and heap tags are consistently replicated across nodes. Clustering is optional and requires HTTP-based transports.

## Why Clustering

In production deployments, a single mcp-v8 server is a single point of failure. If it crashes, in-flight executions are lost, and the session log becomes unavailable until the server restarts. Clustering addresses this by replicating metadata (session logs and heap tags) across multiple nodes, ensuring that the loss of any single node does not lose data.

Note that heap snapshot blobs are not replicated through Raft. They rely on the shared storage backend (S3, shared filesystem) being available to all nodes. Raft replicates the lightweight metadata that references those blobs.

## Leader Election

The cluster uses Raft leader election to establish a single leader at any given time:

1. All nodes start as **followers**.
2. Each follower has a randomized election timeout (configurable via `--election-timeout-min` and `--election-timeout-max`, default: 300-500ms).
3. If a follower does not receive a heartbeat from the leader within its election timeout, it transitions to **candidate** and starts an election.
4. The candidate increments its term, votes for itself, and sends `RequestVote` RPCs to all peers.
5. A peer grants its vote if the candidate's term is at least as large as the peer's current term and the candidate's log is at least as up-to-date as the peer's log.
6. If the candidate receives votes from a majority of nodes, it becomes the **leader**.
7. If another node becomes leader first (detected via a higher-term AppendEntries), the candidate reverts to follower.

## Log Replication

Once a leader is established, all writes go through the Raft log:

1. A client writes (e.g., a session log entry or a tag update) is received by any node.
2. If the receiving node is not the leader, it forwards the write to the leader (write forwarding).
3. The leader appends the entry to its log and sends `AppendEntries` RPCs to all followers.
4. Each follower appends the entry to its local log and acknowledges.
5. Once a majority of nodes have acknowledged, the leader commits the entry and applies it to its local state (sled database).
6. On the next heartbeat, followers learn the new commit index and apply committed entries to their local state.

## Heartbeat and Election Timeouts

The leader sends periodic heartbeats (empty `AppendEntries` RPCs) to all followers. The heartbeat interval defaults to 100ms and is configurable via `--heartbeat-interval`.

The election timeout is randomized within the `[election-timeout-min, election-timeout-max]` range for each node. This randomization prevents split-brain scenarios where multiple nodes start elections simultaneously. The default range (300-500ms) provides a good balance between failure detection speed and stability.

The heartbeat also carries the leader's current peer set, allowing followers to track membership changes without a separate protocol.

## Dynamic Peer Discovery

Nodes can join an existing cluster dynamically without pre-configuring all peers at startup:

1. A new node starts with `--join <seed-address>`.
2. The new node sends a `POST /raft/join` request to the seed node with its own node ID and advertise address.
3. If the seed is the leader, it adds the new peer to its peer set and replicates the membership change. If the seed is a follower, it forwards the join request to the leader.
4. Subsequent heartbeats from the leader carry the updated peer set, so all existing nodes discover the new peer.

Nodes can also leave the cluster via `POST /raft/leave`.

Static peer configuration is also supported via `--peers` with entries in either `host:port` or `id@host:port` format.

## Write Forwarding

Any node in the cluster can receive a write request. If the receiving node is not the leader, it looks up the leader's address (from the Raft state) and forwards the request via HTTP. This transparent forwarding means clients do not need to know which node is the leader.

Write forwarding uses the `peer_addrs` map to resolve the leader's network address. This map is populated from the `--peers` configuration and updated via heartbeat-carried peer sets.

## Session Log Replication

Session logs in cluster mode use a different key scheme than in standalone mode:

- **Session existence** -- Stored as `sl:s:<session_name>` = `"1"`
- **Session entries** -- Stored as `sl:e:<session_name>:<nanosecond_timestamp>` = JSON entry

This key scheme enables prefix scanning (`sl:s:` for session names, `sl:e:<session>:` for entries in a session) and preserves chronological ordering within each session.

Reads use `scan_prefix` on the replicated data tree. Writes go through `put_or_forward` to ensure they pass through the Raft log.

## Sled Persistence Per Node

Each node in the cluster maintains its own sled database at `<session_db_path>/cluster-<node_id>`. This database stores:

- The Raft log (term, index, key, value for each entry)
- The replicated data tree (applied entries)
- Raft persistent state (current term, voted-for)

On startup, nodes recover their state from sled, replaying the log to rebuild the in-memory state. This ensures that a node that crashes and restarts can rejoin the cluster without losing committed data.

## Cluster Mode Requirements

Cluster mode requires:

- `--cluster-port` -- Enables cluster mode and specifies the port for Raft RPCs
- `--http-port` or `--sse-port` -- Required because stdio transport cannot support cluster peer communication
- `--node-id` -- A unique identifier for this node (default: "node1")
- `--peers` or `--join` -- At least one way to discover other cluster members
