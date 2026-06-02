# Content-Addressed Heap Snapshots

In stateful mode, mcp-v8 captures the entire V8 heap after each execution and stores it as a snapshot. These snapshots are the foundation of stateful execution: they allow any prior JavaScript state to be restored and continued from where it left off. The design uses content addressing, immutability, and an integrity envelope to make snapshots reliable and efficient.

## What a Heap Snapshot Contains

A V8 heap snapshot is a binary blob produced by `JsRuntimeForSnapshot::snapshot()`. It contains the serialized state of everything in the JavaScript environment: global variables, function closures, prototype chains, module registrations, and internal V8 data structures. When a new V8 isolate is created with this snapshot as its `startup_snapshot`, it starts in exactly the state the previous execution left behind.

This is a V8-level mechanism -- the same technique that Deno and Node.js use to speed up startup by pre-serializing built-in modules. mcp-v8 repurposes it for user-level state persistence.

## Content Addressing with SHA-256

After V8 produces a snapshot, mcp-v8 computes a SHA-256 hash of the raw snapshot bytes. This hash becomes the storage key -- the "heap ID" that callers use to reference the snapshot.

Content addressing provides several properties:

- **Deterministic identity** -- The same JavaScript state always produces the same hash. If two executions arrive at identical states through different paths, they share a single stored snapshot.
- **Deduplication** -- Storing a snapshot whose hash already exists is a no-op. This matters when agents re-execute the same initialization code, which happens frequently in practice.
- **Immutability** -- Once stored, a snapshot never changes. The hash guarantees that what you retrieve is exactly what was stored. There is no "update" operation on snapshots.
- **Integrity verification** -- Corruption or tampering is detectable by recomputing the hash.

## The Snapshot Envelope

V8's snapshot deserialization calls `abort()` on invalid data -- a fatal error that cannot be caught in Rust. To prevent corrupted or adversarial data from reaching V8, mcp-v8 wraps every snapshot in an envelope that is validated before deserialization.

The envelope format is:

```
[MCPV8SNAP\0]  (10 bytes)  -- Magic header
[SHA-256 hash] (32 bytes)  -- Checksum of the payload
[V8 payload]   (variable)  -- Raw V8 snapshot data
```

Validation checks three things:

1. **Magic header** -- The first 10 bytes must be `MCPV8SNAP\0`. This immediately rejects obviously wrong data (e.g., a JPEG accidentally stored as a heap).
2. **SHA-256 checksum** -- The hash of the payload must match the stored checksum. This catches bit-rot, truncation, and partial writes.
3. **Minimum payload size** -- Valid V8 snapshots are always larger than 100 KB. Payloads smaller than this are rejected. This also prevents fuzz testing from synthesizing valid envelopes with trivially small payloads.

The checksum and payload are stored atomically as a single blob rather than as separate storage keys. This eliminates a class of consistency bugs where the checksum updates but the payload does not (or vice versa).

## Session Logging

Sessions provide a named, ordered log of executions. Each session is identified by a string name (chosen by the caller), and each entry in the log records:

- **input_heap** -- The heap snapshot that was loaded before execution (or null for the first execution in a chain).
- **output_heap** -- The heap snapshot produced after execution.
- **code** -- The JavaScript/TypeScript source that was executed.
- **timestamp** -- When the execution occurred (ISO 8601 UTC).

Session logs are stored in sled. In standalone mode, each session is a separate sled tree, with entries keyed by a monotonically increasing sequence number. In cluster mode, session data is stored in the Raft-replicated data tree using a key scheme that preserves ordering (`sl:e:<session>:<nanosecond_timestamp>`).

## The Heap-to-Session Relationship

Heaps and sessions are independent concepts that reference each other:

- A **heap** is a content-addressed snapshot. It exists in storage regardless of whether any session references it. Multiple sessions can reference the same heap.
- A **session** is an ordered log of (input_heap, code, output_heap) triples. It provides a linear history of how a particular conversation or workflow evolved.

This separation means that branching is natural: an agent can take the output heap from step 3 of session A and use it as the input to a new session B. The heap exists independently; the sessions just record which heaps were used in which order.

## Design Rationale

The content-addressed, immutable design was chosen to match the nature of agent interactions:

- Agents frequently retry or branch. Content addressing means retries that produce the same state do not waste storage.
- Agents need to revisit prior states ("go back to when x was defined"). Any heap hash from the session log is a valid restoration point.
- The system must be robust against crashes. Immutable, checksummed blobs either exist and are valid, or they do not exist. There is no "partially written" state.
- Multi-turn conversations may span long time periods. Content-addressed snapshots are as valid a year later as they were when created.
