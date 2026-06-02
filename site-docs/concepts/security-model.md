# Security Model

mcp-v8 runs untrusted code from AI agents. Its security model is designed around the assumption that every piece of JavaScript submitted for execution could be malicious, malformed, or simply resource-hungry. Multiple layers of defense work together to contain the impact of any single failure.

## V8 Isolation

Each execution runs in its own V8 isolate, which provides process-level memory isolation between executions. V8 isolates do not share heap memory, and one isolate cannot access another's data. This is the same isolation boundary that web browsers use to separate tabs.

Within the isolate, the sandbox is hardened before user code runs:

- `Deno.core.ops` is frozen to prevent interception or replacement of native operations.
- `__bootstrap` is deleted, removing access to event loop hooks, primordials (pristine built-in constructors), and internal registration objects.
- `SharedArrayBuffer` and `Atomics` are removed as a defense-in-depth measure against Spectre-style timing attacks.
- `op_panic` is replaced with a JavaScript function that throws an Error instead of calling Rust's `panic!()`.
- `op_print` is replaced with a function that routes through console capture, preventing direct writes to stdout (which would corrupt the JSON-RPC protocol stream).
- Introspection ops (`op_get_proxy_details`, `op_memory_usage`, `op_is_terminal`) are neutralized.

After hardening, user code cannot access V8 internals, cannot escape the sandbox through prototype chain traversal, and cannot observe or influence other executions.

## Heap Memory Limits

Each V8 isolate is created with a configurable heap limit (default: 8 MB, minimum: 8 MB). The limit is enforced through V8's `heap_limits` parameter on isolate creation.

When the heap approaches its limit, a `near_heap_limit_callback` fires. This callback:

1. Sets an out-of-memory flag.
2. Calls `terminate_execution()` on the isolate.
3. Returns a doubled limit to give V8 room to clean up.

The termination causes V8 to throw an uncatchable exception, which the Rust code catches and reports as "Out of memory: V8 heap limit exceeded."

## Bounded ArrayBuffer Allocator

V8's typed arrays (`Uint8Array`, `Float64Array`, etc.) allocate backing memory through an `ArrayBuffer::Allocator`, which is separate from the managed heap. The default allocator uses `malloc` with no limit -- a script creating large typed arrays could exhaust system memory and cause V8 to call `FatalProcessOutOfMemory`, which terminates the process.

mcp-v8 replaces the default allocator with a bounded allocator that tracks total allocated bytes and returns null when the limit is exceeded. V8 treats a null allocation as a failure and throws a JavaScript `RangeError` instead of aborting. The allocation limit is set to the same value as the heap limit.

The bounded allocator uses atomic operations for thread-safe tracking and proper alignment for all allocations.

## OOM Protection

Despite the heap limit and bounded allocator, certain pathological V8 operations can still trigger a fatal OOM (e.g., `new Array(1e9)` exceeds V8's internal `FixedArray::kMaxLength` before the heap limit callback fires). For these cases, mcp-v8 installs a V8 OOM error handler that logs a descriptive message and calls `abort()`.

The `abort()` is intentional: V8 may hold internal locks and have global state in an inconsistent state after a fatal OOM. Recovery is not possible. The process manager (Docker, systemd, etc.) should restart the server.

The minimum heap limit of 8 MB and the near-heap-limit callback handle the vast majority of OOM scenarios gracefully. The fatal handler is a last resort for pathological cases.

## Execution Timeouts

Every execution races against a configurable timeout (default: 30 seconds, maximum: 300 seconds). The timeout is enforced using `tokio::select!`: if the timer fires before V8 finishes, `IsolateHandle::terminate_execution()` is called.

The V8 isolate handle is thread-safe and can be called from any thread. The termination sets a flag in V8 that causes the engine to throw an uncatchable exception at the next safe point. Combined with `catch_unwind` on the Rust side, this ensures that even infinite loops are terminated.

## OPA Policy Gating for All External Operations

Every operation that reaches outside the V8 sandbox is gated by an OPA policy:

| Operation | Policy domain | Default |
|---|---|---|
| `fetch()` | `fetch` | Unavailable unless configured |
| `fs.*` | `filesystem` | Unavailable unless configured |
| `import "npm:..."` | `modules` | Blocked unless `--allow-external-modules` |
| `mcp.callTool()` | `mcp_tools` | Allowed (if MCP servers configured) |
| subprocess execution | `subprocess` | Unavailable unless configured |

Capabilities are disabled by default and must be explicitly enabled. Even when enabled, policies can restrict operations to specific URLs, paths, packages, tools, or commands.

## Snapshot Envelope Validation

Before any snapshot data reaches V8 for deserialization, it passes through the envelope validation:

1. **Magic header check** -- Rejects data that is not a mcp-v8 snapshot.
2. **SHA-256 checksum verification** -- Rejects corrupted or tampered data.
3. **Minimum payload size** -- Rejects payloads smaller than 100 KB (all valid V8 snapshots are larger).

This three-layer validation prevents V8's `Snapshot::Initialize` from calling `abort()` on invalid data, which would be a denial-of-service vector if an attacker could inject malicious data into the storage backend.

## JWT Verification

When `--jwks-url` is configured, mcp-v8 verifies JWT tokens during the MCP `initialize` handshake:

1. The client includes an `Authorization: Bearer <token>` header.
2. mcp-v8 decodes the JWT header to extract the `kid` (key ID).
3. The corresponding public key is looked up in the cached JWKS key set.
4. If the key is not found, the JWKS endpoint is re-fetched (handling key rotation).
5. The JWT signature is verified against the public key.

Only the signature is verified -- claim validation (expiration, audience, issuer) can be added by the deployment's authentication infrastructure. The verified claims are available for downstream policy decisions.

## Non-Root Docker Execution

The official Docker image runs mcp-v8 as a non-root user. This limits the blast radius if a vulnerability allows code to escape the V8 sandbox: the escaped code runs with the privileges of an unprivileged user, not root.

## Defense in Depth Summary

The security model is intentionally layered. No single mechanism is expected to be perfect:

1. **V8 isolate** -- Memory isolation between executions
2. **Sandbox hardening** -- Neutralization of dangerous APIs within V8
3. **Heap limits + bounded allocator** -- Memory exhaustion prevention
4. **Execution timeouts** -- CPU exhaustion prevention
5. **OPA policies** -- Capability control for external operations
6. **Snapshot validation** -- Integrity protection for deserialized state
7. **JWT verification** -- Authentication for network-exposed deployments
8. **Non-root execution** -- Privilege limitation at the OS level

Each layer addresses a different threat vector. The combination provides reasonable security for running untrusted code in production, with the understanding that no sandbox is perfectly escape-proof and the security boundaries should be complemented by network-level controls (firewalls, network policies) in production deployments.
