# Clustering

Cluster mode adds Raft-inspired coordination for distributed deployments. It
handles leader election, replicated session logging, and write forwarding for
operations that must be coordinated across nodes.

This is an operational scaling feature, not just a transport flag. It only
applies to network transports and introduces peer discovery, node identity, and
timing configuration concerns.
