# In-Memory Heap Storage

## Overview

This repository now includes an in-memory heap storage implementation that stores V8 heap snapshots in RAM rather than persisting them to disk or cloud storage. This is useful for:

- **Testing and Development**: Fast iteration without filesystem I/O overhead
- **Temporary Sessions**: Sessions that don't require persistence beyond process lifetime
- **Performance**: Faster read/write operations compared to disk or S3
- **Simplicity**: No external dependencies or configuration required

## Implementation Details

### Architecture

The `InMemoryHeapStorage` struct uses:
- `HashMap<String, Vec<u8>>` to store heap snapshots keyed by heap name
- `Arc<RwLock<...>>` for thread-safe concurrent access
- Tokio's async RwLock for non-blocking operations

### Features

- **Thread-safe**: Multiple concurrent readers with exclusive writer access
- **Clone-able**: Shares the same underlying storage across clones
- **Zero-configuration**: Works out of the box with no setup
- **Rich API**: Additional utility methods beyond the basic `put`/`get` interface

### API

Basic HeapStorage trait methods:
- `put(name: &str, data: &[u8]) -> Result<(), String>` - Store heap snapshot
- `get(name: &str) -> Result<Vec<u8>, String>` - Retrieve heap snapshot

Additional utility methods:
- `len() -> usize` - Get number of stored items
- `is_empty() -> bool` - Check if storage is empty
- `clear()` - Remove all stored data
- `contains_key(name: &str) -> bool` - Check if key exists
- `remove(name: &str) -> Option<Vec<u8>>` - Remove and return specific item
- `keys() -> Vec<String>` - Get all stored keys

## Usage

### Command Line

Run the server with in-memory storage:

```bash
# With stdio transport (default)
cargo run --bin server -- --in-memory

# With HTTP transport
cargo run --bin server -- --in-memory --http-port 8080

# With SSE transport
cargo run --bin server -- --in-memory --sse-port 8080
```

### Programmatic Usage

```rust
use mcp::heap_storage::{InMemoryHeapStorage, HeapStorage};

// Create new in-memory storage
let storage = InMemoryHeapStorage::new();

// Store data
storage.put("heap1", b"snapshot data").await.unwrap();

// Retrieve data
let data = storage.get("heap1").await.unwrap();

// Check existence
if storage.contains_key("heap1").await {
    println!("Found heap1");
}

// Get all keys
let keys = storage.keys().await;
println!("Stored heaps: {:?}", keys);

// Clear all data
storage.clear().await;
```

### Integration with AnyHeapStorage

```rust
use mcp::heap_storage::{AnyHeapStorage, InMemoryHeapStorage};

let storage = AnyHeapStorage::InMemory(InMemoryHeapStorage::new());

// Use through the HeapStorage trait
storage.put("key", b"value").await.unwrap();
let value = storage.get("key").await.unwrap();
```

## Comparison with Other Storage Backends

| Feature | In-Memory | File | S3 |
|---------|-----------|------|-----|
| Persistence | Process lifetime only | Permanent | Permanent |
| Performance | Fastest | Fast | Slower (network) |
| Setup Required | None | Directory path | AWS credentials + bucket |
| Memory Usage | High (all in RAM) | Low | Low |
| Distributed Access | No | Shared filesystem only | Yes |
| Best For | Testing, dev, temp sessions | Single server production | Multi-server production |

## Configuration Options

The in-memory storage conflicts with other storage options:
- Cannot be used with `--s3-bucket`
- Cannot be used with `--directory-path`
- Cannot be used with `--stateless`

## Limitations

1. **No Persistence**: All data is lost when the process terminates
2. **Memory Bounded**: Limited by available RAM
3. **Single Process**: Cannot be shared across multiple server instances
4. **No Backup**: No automatic snapshot or recovery mechanism

## Testing

The implementation includes comprehensive unit tests:

```bash
# Run all tests
cargo test -p server

# Run only heap_storage tests
cargo test -p server heap_storage
```

Test coverage includes:
- Basic put/get operations
- Error handling for missing keys
- Utility method functionality
- Clone behavior (shared storage)
- Integration with AnyHeapStorage enum

## When to Use

**Use in-memory storage when:**
- Developing and testing locally
- Running ephemeral workloads
- Performance is critical and persistence is not
- You want zero configuration overhead

**Don't use in-memory storage when:**
- You need data to survive process restarts
- Running in production with important data
- Memory is constrained
- Multiple servers need to share state

## Future Enhancements

Possible improvements:
- Memory usage limits with LRU eviction
- Periodic snapshots to disk
- Memory metrics and monitoring
- TTL (time-to-live) for entries
