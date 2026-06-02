# Tutorial: Setting Up a Multi-Node Cluster

In this tutorial you will set up a 3-node mcp-v8 cluster using the Raft consensus protocol. You will start three nodes, observe leader election, execute code across the cluster, and test failover.

## Prerequisites

- mcp-v8 installed ([Installation](installation.md))
- mcp-v8-cli installed
- Three free terminal windows (or ability to run background processes)

## Step 1: Understand clustering in mcp-v8

mcp-v8 uses the Raft consensus protocol for clustering. This provides:

- **Leader election**: One node is automatically elected as the leader
- **State replication**: Heap snapshots are replicated across nodes
- **Fault tolerance**: The cluster continues operating if a minority of nodes fail
- **Consistency**: All reads and writes go through the leader

A 3-node cluster tolerates 1 node failure. A 5-node cluster tolerates 2 failures.

## Step 2: Plan your cluster

For this tutorial, you will run all three nodes on localhost with different ports:

| Node | Node ID | HTTP Port | Cluster Port |
|---|---|---|---|
| Node 1 | 1 | 3001 | 5001 |
| Node 2 | 2 | 3002 | 5002 |
| Node 3 | 3 | 3003 | 5003 |

Each node needs:
- A unique `--node-id`
- A `--cluster-port` for Raft communication
- A list of `--peers` pointing to the other nodes

## Step 3: Create storage directories

```bash
mkdir -p /tmp/mcp-v8-cluster/node1
mkdir -p /tmp/mcp-v8-cluster/node2
mkdir -p /tmp/mcp-v8-cluster/node3
```

## Step 4: Start Node 1

In the first terminal:

```bash
mcp-v8 \
  --http-port 3001 \
  --node-id 1 \
  --cluster-port 5001 \
  --peers "2=127.0.0.1:5002,3=127.0.0.1:5003" \
  --directory-path /tmp/mcp-v8-cluster/node1
```

Node 1 starts and begins trying to contact its peers for leader election.

## Step 5: Start Node 2

In the second terminal:

```bash
mcp-v8 \
  --http-port 3002 \
  --node-id 2 \
  --cluster-port 5002 \
  --peers "1=127.0.0.1:5001,3=127.0.0.1:5003" \
  --directory-path /tmp/mcp-v8-cluster/node2
```

## Step 6: Start Node 3

In the third terminal:

```bash
mcp-v8 \
  --http-port 3003 \
  --node-id 3 \
  --cluster-port 5003 \
  --peers "1=127.0.0.1:5001,2=127.0.0.1:5002" \
  --directory-path /tmp/mcp-v8-cluster/node3
```

Once a majority of nodes (2 out of 3) are running, leader election completes. Watch the terminal output to see which node becomes the leader.

## Step 7: Execute code on the cluster

Send a request to any node. If it is not the leader, it will forward to the leader:

```bash
mcp-v8-cli --http-port 3001 exec --code '
var clusterTest = "Hello from the cluster!";
clusterTest;
'
```

Expected output:

```
Hello from the cluster!
```

## Step 8: Verify state replication

The heap snapshot is replicated to all nodes. Query a different node:

```bash
mcp-v8-cli --http-port 3002 exec --code '
clusterTest;
' --heap-id <HEAP_HASH_FROM_STEP_7>
```

Expected output:

```
Hello from the cluster!
```

The state is available on Node 2 even though the code was executed via Node 1.

## Step 9: Build up state across nodes

Continue adding state, sending requests to different nodes:

```bash
# Execute on Node 2
mcp-v8-cli --http-port 3002 exec --code '
var items = ["first"];
items;
' --heap-id <HEAP_HASH_FROM_STEP_7>

# Execute on Node 3 (using the hash from the previous step)
mcp-v8-cli --http-port 3003 exec --code '
items.push("second");
items;
' --heap-id <HEAP_HASH_FROM_STEP_8>
```

The state flows consistently through the cluster regardless of which node you connect to.

## Step 10: Test failover

Now simulate a node failure. Stop Node 1 by pressing `Ctrl+C` in its terminal.

The remaining two nodes (Node 2 and Node 3) still form a majority, so the cluster continues operating:

```bash
mcp-v8-cli --http-port 3002 exec --code '
"Cluster is still running with 2 nodes!";
'
```

Expected output:

```
Cluster is still running with 2 nodes!
```

## Step 11: Verify continued operation

Execute more code and verify the cluster handles it:

```bash
mcp-v8-cli --http-port 3003 exec --code '
var failoverTest = "This was written while one node was down";
failoverTest;
'
```

The cluster remains fully functional.

## Step 12: Restore the failed node

Restart Node 1:

```bash
mcp-v8 \
  --http-port 3001 \
  --node-id 1 \
  --cluster-port 5001 \
  --peers "2=127.0.0.1:5002,3=127.0.0.1:5003" \
  --directory-path /tmp/mcp-v8-cluster/node1
```

Node 1 rejoins the cluster and catches up on any state it missed while it was down.

## Step 13: Verify the restored node

Query Node 1 for data that was written while it was down:

```bash
mcp-v8-cli --http-port 3001 exec --code '
failoverTest;
' --heap-id <HEAP_HASH_FROM_STEP_11>
```

Expected output:

```
This was written while one node was down
```

The restored node has all the data.

## What you learned

- How to configure a 3-node Raft cluster with `--node-id`, `--cluster-port`, and `--peers`
- That leader election happens automatically when a majority of nodes are running
- That requests to any node are forwarded to the leader
- That heap snapshots are replicated across all nodes
- That the cluster tolerates minority node failures (1 out of 3)
- That failed nodes catch up automatically when they rejoin

Next, learn about storage options in [Configuring Heap Storage](storage-backends.md).
