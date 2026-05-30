# Clustering

Cluster mode adds distributed coordination to `mcp-v8` for deployments that
need replicated session state and coordinated writes across multiple nodes.

It is not just a transport flag. Cluster mode changes how the server handles
leadership, write ownership, and operational topology.

```mermaid
flowchart LR
  A[Client request] --> B[Follower or leader node]
  B --> C{leader?}
  C -->|yes| D[append coordinated write]
  C -->|no| E[forward write to leader]
  E --> D
  D --> F[replicate session log]
  F --> G[cluster state converges]
```

The cluster layer is Raft-inspired. It handles:

- leader election
- replicated session logging
- write forwarding when a request lands on a follower
- peer discovery and node identity concerns

This matters most for stateful, networked deployments. It does not apply to
stdio, and it introduces configuration concerns such as `--cluster-port`,
`--node-id`, peer lists, advertise addresses, and timing parameters.

Cluster mode is an operational scaling feature. It helps preserve a coherent
view of state across nodes, but it also makes deployment and failure handling
more complex than single-node operation.
