# Run a Cluster

Start a small three-node `mcp-v8` cluster on one machine.

This setup uses:

- Streamable HTTP for the MCP and REST surfaces
- local filesystem storage for heaps
- separate session databases per node
- a dedicated Raft port per node

## 1. Create per-node storage directories

```bash
mkdir -p /tmp/mcp-v8-cluster/node1/heaps /tmp/mcp-v8-cluster/node2/heaps /tmp/mcp-v8-cluster/node3/heaps
mkdir -p /tmp/mcp-v8-cluster/node1/db /tmp/mcp-v8-cluster/node2/db /tmp/mcp-v8-cluster/node3/db
```

Each node needs its own local heap directory and session database path.

## 2. Start the seed node

Start the first node with both peer addresses listed up front:

```bash
mcp-v8 \
  --directory-path /tmp/mcp-v8-cluster/node1/heaps \
  --session-db-path /tmp/mcp-v8-cluster/node1/db/sessions \
  --http-port 3001 \
  --cluster-port 4001 \
  --node-id node1 \
  --advertise-addr 127.0.0.1:4001 \
  --peers node2@127.0.0.1:4002,node3@127.0.0.1:4003
```

`--cluster-port` enables the Raft server. Because cluster mode forwards writes
between nodes, `--advertise-addr` should be a routable address that other nodes
can use.

## 3. Start the two joining nodes

Node 2:

```bash
mcp-v8 \
  --directory-path /tmp/mcp-v8-cluster/node2/heaps \
  --session-db-path /tmp/mcp-v8-cluster/node2/db/sessions \
  --http-port 3002 \
  --cluster-port 4002 \
  --node-id node2 \
  --advertise-addr 127.0.0.1:4002 \
  --join 127.0.0.1:4001
```

Node 3:

```bash
mcp-v8 \
  --directory-path /tmp/mcp-v8-cluster/node3/heaps \
  --session-db-path /tmp/mcp-v8-cluster/node3/db/sessions \
  --http-port 3003 \
  --cluster-port 4003 \
  --node-id node3 \
  --advertise-addr 127.0.0.1:4003 \
  --join 127.0.0.1:4001
```

`--join` contacts an existing cluster node, which forwards the join request to
the current leader if needed.

## 4. Check cluster status

Each node exposes Raft status on its cluster port:

```bash
curl -s http://127.0.0.1:4001/raft/status | jq
curl -s http://127.0.0.1:4002/raft/status | jq
curl -s http://127.0.0.1:4003/raft/status | jq
```

Look for:

- one node with a leader role
- all three peer addresses in the peer list
- the same leader ID across nodes

## 5. Send traffic to any HTTP node

Once the cluster is up, clients can talk to any node's HTTP port:

```bash
curl -s http://127.0.0.1:3002/api/exec \
  -H 'content-type: application/json' \
  -d '{"code":"console.log(\"cluster ok\")"}'
```

If the request lands on a follower and needs a coordinated write, the follower
forwards it to the leader.

## 6. Understand the routing model

- use `3001`, `3002`, and `3003` for MCP or REST traffic
- use `4001`, `4002`, and `4003` for cluster coordination only
- reads and status checks can hit any node
- leader-owned writes may be forwarded under the hood

## 7. Add a node manually

`--join` is the simplest path, but the cluster API also exposes a direct join
endpoint:

```bash
curl -s http://127.0.0.1:4001/raft/join \
  -H 'content-type: application/json' \
  -d '{"node_id":"node4","addr":"127.0.0.1:4004"}'
```

That request can be sent to any cluster node. Followers forward it to the
leader.

## What to watch in production

- every node needs a unique `--node-id`
- every node needs a unique `--cluster-port`
- `--advertise-addr` must be reachable by peers, not just valid locally
- keep separate heap and session storage per node
- cluster mode adds leader election and quorum concerns to normal server
  operations

For the underlying coordination model, see
[Clustering](../concepts/clustering.md). For the flag reference, see
[CLI Flags](../reference/cli-flags.md).
