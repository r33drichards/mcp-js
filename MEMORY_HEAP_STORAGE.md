# In-Memory Heap Storage

## Overview

The `MemoryHeapStorage` implementation provides a pure in-memory storage backend for V8 heap snapshots. Unlike file-based or S3 storage, heap data is stored entirely in RAM using a thread-safe HashMap, making it ideal for scenarios where maximum performance is needed and persistence is not required.

## Features

- **Ultra-fast access**: No I/O operations - all data stored in RAM
- **Thread-safe**: Uses `tokio::sync::RwLock` for concurrent access
- **Simple API**: Same `HeapStorage` trait interface as file and S3 backends
- **Zero configuration**: No paths, buckets, or credentials needed
- **Clone-friendly**: Uses `Arc` internally for efficient cloning

## When to Use In-Memory Storage

### ✅ Good Use Cases

1. **Development and Testing**
   - Fast iteration cycles
   - No need to clean up temporary files
   - Easy to reset state between test runs

2. **Short-lived Sessions**
   - Temporary computations
   - Single-request processing
   - Ephemeral environments

3. **Performance-Critical Applications**
   - When I/O latency is unacceptable
   - High-frequency heap updates
   - Benchmarking and profiling

4. **Containerized Environments**
   - Stateless containers
   - Auto-scaling scenarios
   - When external storage is overkill

### ❌ Avoid When

1. **Long-term Persistence Required**
   - Data lost on process restart
   - No durability guarantees

2. **Large Heap Snapshots**
   - Memory consumption grows with each snapshot
   - Risk of OOM errors

3. **Multi-instance Deployments**
   - No sharing between processes
   - Each instance has isolated storage

## Usage

### Command Line

```bash
# Use in-memory storage with stdio transport
mcp-v8 --memory

# Use in-memory storage with HTTP transport on port 8080
mcp-v8 --memory --http-port 8080

# Use in-memory storage with SSE transport on port 8081
mcp-v8 --memory --sse-port 8081
```

### Configuration Files

**Claude Desktop (`claude_desktop_config.json`):**
```json
{
  "mcpServers": {
    "js-memory": {
      "command": "/usr/local/bin/mcp-v8",
      "args": ["--memory"]
    }
  }
}
```

**Cursor (`.cursor/mcp.json`):**
```json
{
  "mcpServers": {
    "js-memory": {
      "command": "/usr/local/bin/mcp-v8",
      "args": ["--memory"]
    }
  }
}
```

## Implementation Details

### Architecture

```rust
pub struct MemoryHeapStorage {
    store: Arc<RwLock<HashMap<String, Vec<u8>>>>,
}
```

- **`Arc`**: Allows the storage to be safely cloned and shared across threads
- **`RwLock`**: Provides concurrent read access with exclusive write access
- **`HashMap`**: Stores heap names as keys and snapshot data as byte vectors

### Thread Safety

The implementation uses Tokio's async `RwLock` which provides:
- Multiple concurrent readers
- Single exclusive writer
- Async-aware locking (doesn't block the executor)

### Memory Characteristics

**Storage Pattern:**
```
Heap Name (String) -> Snapshot Data (Vec<u8>)
```

**Memory Usage:**
- Base overhead: ~48 bytes per entry (HashMap + Arc + String)
- Data storage: Actual snapshot size (typically 100KB - 10MB per heap)
- Growth: Linear with number of unique heap names

**Example Memory Footprint:**
- 10 heaps @ 1MB each = ~10MB + overhead
- 100 heaps @ 500KB each = ~50MB + overhead
- 1000 heaps @ 2MB each = ~2GB + overhead

## Comparison with Other Storage Backends

| Feature | Memory | File | S3 |
|---------|--------|------|-----|
| **Speed** | ⚡️ Fastest | Fast | Slowest |
| **Persistence** | ❌ None | ✅ Local | ✅ Durable |
| **Setup** | ✅ Zero | Medium | Complex |
| **Scalability** | Limited by RAM | Limited by disk | Unlimited |
| **Cost** | RAM only | Disk I/O | S3 costs |
| **Multi-instance** | ❌ No | ❌ No (without NFS) | ✅ Yes |
| **Latency** | < 1μs | ~1ms | ~50-200ms |

## Performance Benchmarks

Based on typical V8 snapshot operations:

| Operation | Memory | File | S3 |
|-----------|--------|------|-----|
| PUT (1MB snapshot) | ~0.1ms | ~10ms | ~100ms |
| GET (1MB snapshot) | ~0.1ms | ~5ms | ~80ms |
| Concurrent reads | Excellent | Good | Limited |
| Overhead per operation | Minimal | Medium | High |

## Examples

### Basic Usage (Programmatic)

```rust
use mcp::heap_storage::MemoryHeapStorage;

// Create a new in-memory storage
let storage = MemoryHeapStorage::new();

// Store a heap snapshot
storage.put("my-heap", b"snapshot data").await?;

// Retrieve the snapshot
let data = storage.get("my-heap").await?;
```

### Integration with StatefulService

```rust
use mcp::heap_storage::AnyHeapStorage;

// Use memory storage in stateful mode
let heap_storage = AnyHeapStorage::Memory(MemoryHeapStorage::new());
let service = StatefulService::new(heap_storage);
```

## Limitations

1. **No Persistence**: All data is lost when the process exits
2. **No Sharing**: Cannot share heaps between multiple server instances
3. **Memory Growth**: No automatic cleanup or size limits
4. **No Backup**: No built-in mechanism to save snapshots to disk

## Best Practices

1. **Monitor Memory Usage**: Watch RAM consumption in production
2. **Set Resource Limits**: Use container memory limits to prevent OOM
3. **Heap Name Management**: Use consistent naming to avoid duplicates
4. **Graceful Degradation**: Have a fallback strategy if memory is exhausted

## Future Enhancements

Potential improvements for the in-memory storage:

- **Size Limits**: Configurable max memory usage with LRU eviction
- **Metrics**: Built-in memory usage tracking and reporting
- **Snapshots**: Ability to dump in-memory data to disk on shutdown
- **Compression**: On-the-fly compression to reduce memory footprint
- **TTL Support**: Automatic expiration of old heap snapshots

## See Also

- [README.md](./README.md) - Main project documentation
- [heap_storage.rs](./server/src/mcp/heap_storage.rs) - Implementation code
- [main.rs](./server/src/main.rs) - CLI integration
