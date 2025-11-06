# Heap Storage Options

The MCP V8 server supports multiple heap storage backends for persisting V8 snapshots between JavaScript executions.

## Storage Backends

### 1. In-Memory Storage (New! 🎉)

Perfect for debugging and development. All heap snapshots are stored in RAM using a thread-safe HashMap.

**Usage:**
```bash
cargo run -- --in-memory
cargo run -- --in-memory --sse-port 3000
```

**Features:**
- ⚡ Lightning fast - no disk I/O
- 🔍 Perfect for debugging - easy to inspect
- 🧪 Great for testing - no cleanup needed
- 🚀 Thread-safe with Arc<RwLock<HashMap>>
- 📊 Includes helper methods: `len()`, `is_empty()`, `clear()`, `list_heaps()`

**Limitations:**
- Data is lost when the process exits
- Limited by available RAM

### 2. File-Based Storage

Stores heap snapshots as files on disk.

**Usage:**
```bash
cargo run -- --directory-path /path/to/heaps
cargo run  # defaults to /tmp/mcp-v8-heaps
```

**Features:**
- 💾 Persistent across restarts
- 📁 Easy to backup and inspect
- 🔄 Good for development and production

### 3. S3 Storage

Stores heap snapshots in AWS S3.

**Usage:**
```bash
cargo run -- --s3-bucket my-heap-bucket
```

**Features:**
- ☁️ Cloud-native storage
- 🌍 Accessible from anywhere
- 📈 Scales automatically
- 🔒 AWS security and durability

**Requirements:**
- AWS credentials configured (environment variables or AWS config)
- S3 bucket must exist

### 4. Stateless Mode

No heap persistence - each execution starts fresh.

**Usage:**
```bash
cargo run -- --stateless
```

**Features:**
- 🏃 Fastest startup
- 🧹 No state management
- ✨ Clean slate every time

## Implementation Details

### InMemoryHeapStorage

The in-memory storage implementation uses:
- `Arc<RwLock<HashMap<String, Vec<u8>>>>` for thread-safe concurrent access
- Async read/write operations
- Clone-friendly for use across async tasks

```rust
// Example: Creating an in-memory store
let storage = InMemoryHeapStorage::new();

// Store a heap
storage.put("my-heap", &snapshot_bytes).await?;

// Retrieve a heap
let snapshot = storage.get("my-heap").await?;

// List all heaps
let heaps = storage.list_heaps().await;

// Clear all heaps
storage.clear().await;
```

### AnyHeapStorage Enum

All storage backends implement the `HeapStorage` trait and are wrapped in the `AnyHeapStorage` enum:

```rust
pub enum AnyHeapStorage {
    File(FileHeapStorage),
    S3(S3HeapStorage),
    InMemory(InMemoryHeapStorage),
}
```

This allows runtime selection of the storage backend based on CLI flags.

## Debugging Tips

When debugging with `--in-memory`:

1. **Monitor heap count**: The storage logs when heaps are stored
2. **Fast iteration**: No disk cleanup between runs
3. **Memory profiling**: Easy to track memory usage
4. **State inspection**: All state is in RAM, accessible via debugger

## CLI Examples

```bash
# In-memory with stdio transport
cargo run -- --in-memory

# In-memory with SSE transport on port 3000
cargo run -- --in-memory --sse-port 3000

# In-memory with HTTP transport on port 8080
cargo run -- --in-memory --http-port 8080

# File-based (default location)
cargo run

# File-based (custom location)
cargo run -- --directory-path ./my-heaps

# S3 backend
cargo run -- --s3-bucket my-production-heaps

# Stateless mode
cargo run -- --stateless
```

## Performance Comparison

| Backend | Read Speed | Write Speed | Persistence | Use Case |
|---------|-----------|-------------|-------------|----------|
| In-Memory | ⚡⚡⚡ | ⚡⚡⚡ | ❌ | Development, Testing, Debugging |
| File | ⚡⚡ | ⚡⚡ | ✅ | Development, Small Production |
| S3 | ⚡ | ⚡ | ✅ | Production, Multi-region |
| Stateless | ⚡⚡⚡ | N/A | ❌ | Stateless workloads |

## When to Use Each Backend

**In-Memory**: 
- Debugging MCP server behavior
- Running tests
- Quick experiments
- When you don't need persistence

**File-Based**:
- Local development
- Single-server deployments
- When you want to inspect heap snapshots manually

**S3**:
- Production deployments
- Multi-server setups
- When you need durability and backup

**Stateless**:
- One-off JavaScript executions
- When you don't need heap persistence
- Maximum performance for independent executions
