# Execution Model

mcp-v8 supports two fundamentally different execution modes -- stateful and stateless -- and uses an asynchronous submit-poll-read pattern for all executions. Understanding these modes and the execution lifecycle is essential for designing effective agent workflows.

## Stateful Mode

In stateful mode, every execution produces a V8 heap snapshot. This snapshot captures the entire state of the JavaScript environment: all variables, closures, prototypes, and module registrations. The snapshot is serialized, hashed with SHA-256 to produce a content-addressed key, and stored in the configured storage backend.

Subsequent executions can provide a `heap` parameter pointing to a previous snapshot. The V8 isolate is restored from that snapshot, and the new code runs in the context of the prior state. This creates a chain of execution states, where each step builds on the last.

Stateful mode exists because multi-turn agent conversations benefit enormously from persistent state. An agent can define functions in one turn, populate data structures in the next, and query them in a third -- without re-running all prior code. The content-addressed storage means that identical states are automatically deduplicated, and any prior state can be revisited by referencing its hash.

The trade-off is that V8's `SnapshotCreator` is not safe to run concurrently. mcp-v8 uses a Tokio mutex to serialize snapshot creation, meaning stateful executions run one at a time (though the I/O-bound work of loading/storing snapshots happens outside this lock). Stateless executions are unaffected and run in full parallelism.

## Stateless Mode

In stateless mode, each execution starts from a fresh V8 isolate. No snapshots are created or loaded. This mode is simpler, faster for one-shot evaluations, and fully parallelizable.

Stateless mode is appropriate when executions are independent -- for example, evaluating mathematical expressions, transforming data, or running self-contained scripts. Without the overhead of snapshot serialization and storage, stateless mode can handle higher throughput.

## Why Two Modes Exist

The two modes reflect a fundamental trade-off between state persistence and concurrency. Stateful mode is powerful for iterative workflows but serializes V8 execution. Stateless mode sacrifices state persistence for throughput. The choice is made at server startup and applies to all executions on that server instance.

In practice, many deployments run both: a stateful instance for multi-turn agent sessions and a stateless instance (or pool) for one-shot evaluations.

## Async Execution: Submit, Poll, Read

All executions -- stateful and stateless -- follow an asynchronous pattern:

### 1. Submit

The caller sends code (via MCP tool call or HTTP POST). The server immediately returns an execution ID without waiting for the code to finish. This non-blocking submission means the caller (typically an AI agent) is never stuck waiting for a long-running script.

### 2. Poll

The caller checks execution status by ID. The possible statuses are:

- **running** -- The code is still executing. Console output may already be available for streaming.
- **completed** -- Execution finished successfully. The result and (in stateful mode) the output heap snapshot hash are available.
- **failed** -- Execution encountered an error. The error message describes what went wrong.
- **cancelled** -- The execution was explicitly cancelled by the caller.
- **timed_out** -- The execution exceeded its time limit and was forcibly terminated.

### 3. Read

Once complete, the caller retrieves the result and any console output. Console output can also be read while execution is still running, enabling streaming use cases.

This pattern was chosen over synchronous execution for several reasons:

- Long-running scripts do not block the MCP transport.
- Console output can be streamed incrementally.
- Executions can be cancelled mid-flight.
- Multiple executions can be in flight simultaneously.
- The timeout mechanism works cleanly with the async model.

## Concurrency Control

V8 isolates are heavyweight: each one allocates a managed heap, runs on a blocking thread, and consumes significant memory. Unbounded concurrency would exhaust OS threads and memory.

mcp-v8 uses a Tokio semaphore to limit concurrent V8 executions. The default concurrency limit equals the number of CPU cores, but it can be configured via `--max-concurrent-executions`. When all permits are taken, new executions wait in a queue until a permit becomes available.

This semaphore applies to both stateful and stateless executions. In stateful mode, the snapshot mutex provides an additional serialization point, but the semaphore is still the primary mechanism for bounding resource usage.

## Timeout Enforcement

Every execution races against a configurable timeout (default: 30 seconds, maximum: 300 seconds). The timeout mechanism uses `tokio::select!` to race the V8 task against a sleep timer:

- If the V8 task completes first, the timeout is cancelled.
- If the timer fires first, `IsolateHandle::terminate_execution()` is called. This sets a V8-internal flag that causes the engine to throw an uncatchable exception at the next safe point. The V8 task then completes with a "timed out" error.

The V8 isolate handle is published to a shared `Arc<Mutex<Option<IsolateHandle>>>` as soon as the isolate is created, making it available for both timeout termination and explicit cancellation.

## Execution Registry

All active and recently completed executions are tracked in an `ExecutionRegistry`. This is an in-memory `DashMap` (a concurrent hash map) backed by per-execution sled trees for console output. The registry provides:

- Status tracking across the execution lifecycle
- Isolate handle storage for cancellation
- Console output storage and paginated retrieval
- Listing of all known executions

The registry is ephemeral -- it does not survive server restarts. For durable execution history, the session log (in stateful mode) provides a persistent record.
