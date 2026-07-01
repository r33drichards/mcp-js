# Clustering & replication (Raft)

This guide covers the common tasks for running mcp-v8 in a multi-node Raft cluster: configuring individual nodes, joining an existing cluster, fronting the cluster with a load balancer, and tuning timing parameters.

## Configure a node with cluster flags

Cluster mode activates when `--cluster-port` is provided. You must also supply `--http-port` or `--sse-port`; stdio transport is not supported in cluster mode.

```bash
mcp-v8 \
  --http-port=3000 \
  --cluster-port=4000 \
  --node-id=node1 \
  --peers=node2@node2:4000,node3@node3:4000
```

Key flags:

| Flag | Purpose |
|------|---------|
| `--cluster-port` | Port the Raft HTTP server listens on (required to enable cluster mode) |
| `--node-id` | Unique identifier for this node (default `node1`) |
| `--peers` | Comma-separated list of peer addresses in `id@host:port` format |

Each peer entry in `--peers` gives the node a name and a Raft address. A node does not need to list itself — only the other members.

For a three-node cluster, run three processes with symmetric peer lists:

```bash
# node1
mcp-v8 --http-port=3000 --cluster-port=4000 --node-id=node1 \
       --peers=node2@node2:4000,node3@node3:4000

# node2
mcp-v8 --http-port=3000 --cluster-port=4000 --node-id=node2 \
       --peers=node1@node1:4000,node3@node3:4000

# node3
mcp-v8 --http-port=3000 --cluster-port=4000 --node-id=node3 \
       --peers=node1@node1:4000,node2@node2:4000
```

## Set the advertise address

By default each node advertises itself as `{node_id}:{cluster_port}` to its peers. If that name is not resolvable from peer machines — for example when running on bare metal with IP addresses — override it with `--advertise-addr`:

```bash
mcp-v8 --http-port=3000 --cluster-port=4000 --node-id=node1 \
       --advertise-addr=10.0.0.1:4000 \
       --peers=node2@10.0.0.2:4000,node3@10.0.0.3:4000
```

The advertise address is stored in the Raft metadata and propagated to followers via heartbeats, so all members stay in sync about peer locations.

## Join an existing cluster

Use `--join` to add a new node to a running cluster. The `--join` value is the `host:port` of any existing cluster member (the request is forwarded to the leader automatically):

```bash
mcp-v8 --http-port=3000 --cluster-port=4000 --node-id=node4 \
       --advertise-addr=node4:4000 \
       --join=node1:4000
```

At startup the node sends a `POST /raft/join` to the seed address. The leader registers the new peer and begins replicating the log to it. There is no need to restart the existing nodes.

## Put a load balancer in front

Point the load balancer at each node's MCP port (not the cluster port). The cluster port is for inter-node Raft traffic only and should not be exposed to clients.

The following nginx snippet matches the setup in `nginx-cluster.conf`:

```nginx
upstream mcp_cluster {
    server node1:3000;
    server node2:3000;
    server node3:3000;
}

server {
    listen 8080;

    location /mcp {
        proxy_pass         http://mcp_cluster;
        proxy_http_version 1.1;
        proxy_set_header   Host $host;
        proxy_set_header   Connection '';
        proxy_buffering    off;
        proxy_cache        off;
        proxy_read_timeout 3600s;
        proxy_send_timeout 3600s;
    }

    location /health {
        proxy_pass         http://mcp_cluster/raft/status;
        proxy_http_version 1.1;
    }
}
```

`proxy_buffering off` and `proxy_set_header Connection ''` are required to keep SSE streams and Streamable HTTP responses intact. The `/health` location provides a convenient readiness check that returns the Raft status of whichever backend node nginx selects.

## Tune heartbeat and election timeouts

The defaults (100 ms heartbeat, 300–500 ms election window) suit low-latency local networks. For containers or cloud environments with variable latency, increase all three values as shown in `docker-compose.cluster.yml`:

```bash
mcp-v8 --http-port=3000 --cluster-port=4000 --node-id=node1 \
       --peers=node2@node2:4000,node3@node3:4000 \
       --heartbeat-interval=200 \
       --election-timeout-min=1000 \
       --election-timeout-max=2000
```

Guidelines:

- `--heartbeat-interval` should be well below `--election-timeout-min` so followers receive heartbeats before their timers fire. A ratio of 1:5 or greater is recommended.
- `--election-timeout-min` and `--election-timeout-max` define the random window from which each follower picks its timer. A wider window reduces split-vote collisions.
- All three values are in milliseconds.

## Check cluster status

Query the Raft status of any node directly on its cluster port:

```bash
curl -s http://node1:4000/raft/status | jq '{role, leader_id, term, commit_index}'
```

A healthy cluster has exactly one node with `"role": "Leader"`. All nodes should converge on the same `commit_index` within a few heartbeat intervals.

## Persist the cluster database

Each node stores its Raft metadata (current term, voted-for, log, peer list) in a sled database at `{session-db-path}/cluster-{node-id}`. With the default `--session-db-path=/tmp/mcp-v8-sessions`, node1's cluster DB is at `/tmp/mcp-v8-sessions/cluster-node1`.

Mount a persistent volume at `--session-db-path` so a restarting node can rejoin the cluster without losing its log:

```bash
mcp-v8 --http-port=3000 --cluster-port=4000 --node-id=node1 \
       --session-db-path=/var/lib/mcp-v8/sessions \
       --peers=node2@node2:4000,node3@node3:4000
```

## See also

- [Concepts: Clustering & replication (Raft)](../concepts/clustering.md)
- [Reference: Clustering & replication (Raft)](../reference/clustering.md)
- [Concepts: Heap storage backends](../concepts/storage-backends.md)
- [Concepts: Transports](../concepts/transports.md)
- [Concepts: Stateful sessions & heap snapshots](../concepts/sessions-and-heaps.md)
- [Reference: CLI flags](../reference/cli-flags.md)
