# How to Run a Multi-Node Cluster

Deploy mcp-v8 as a Raft-based cluster for high availability and load distribution.

## Start a 3-node cluster

Cluster mode requires `--http-port` and `--cluster-port`.

**Node 1:**

```bash
mcp-v8 --http-port 3000 \
       --directory-path /data/heaps \
       --session-db-path /data/sessions \
       --cluster-port 4000 \
       --node-id node1 \
       --peers node2@host2:4000,node3@host3:4000
```

**Node 2:**

```bash
mcp-v8 --http-port 3000 \
       --directory-path /data/heaps \
       --session-db-path /data/sessions \
       --cluster-port 4000 \
       --node-id node2 \
       --peers node1@host1:4000,node3@host3:4000
```

**Node 3:**

```bash
mcp-v8 --http-port 3000 \
       --directory-path /data/heaps \
       --session-db-path /data/sessions \
       --cluster-port 4000 \
       --node-id node3 \
       --peers node1@host1:4000,node2@host2:4000
```

## Join an existing cluster dynamically

Instead of listing all peers at startup, join via a seed node:

```bash
mcp-v8 --http-port 3000 \
       --directory-path /data/heaps \
       --cluster-port 4000 \
       --node-id node4 \
       --join host1:4000
```

The node registers itself with the cluster leader via `POST /raft/join`.

## Set the advertise address

If nodes are behind NAT or in containers, set the address other nodes use to reach this one:

```bash
mcp-v8 --cluster-port 4000 --node-id node1 --advertise-addr public-host:4000
```

Defaults to `<node-id>:<cluster-port>`.

## Tune Raft timing

```bash
mcp-v8 --cluster-port 4000 \
       --heartbeat-interval 200 \
       --election-timeout-min 1000 \
       --election-timeout-max 2000
```

| Flag | Default | Description |
|------|---------|-------------|
| `--heartbeat-interval` | 100 ms | Leader heartbeat interval |
| `--election-timeout-min` | 300 ms | Minimum election timeout |
| `--election-timeout-max` | 500 ms | Maximum election timeout |

For cross-datacenter deployments, increase timeouts to account for network latency.

## Deploy with Docker Compose

See [Docker Deployment](docker-deployment.md) for ready-made compose files.

## Stateless cluster

For maximum throughput without heap persistence:

```bash
mcp-v8 --http-port 3000 --stateless --cluster-port 4000 --node-id node1 --peers ...
```
