# Heap Persistence Integration Tests

## Overview

This test suite (`heap_persistence_integration.rs`) was created to reproduce and diagnose production bugs in the MCP server's heap persistence functionality.

## Production Bugs Being Reproduced

### Bug 1: "Heap already exists" False Positive

**Production Error:**
```json
{
  "heap_uri": "file://my-demo-heap.snapshot",
  "message": "Heap 'file://my-demo-heap.snapshot' already exists."
}
```

**Expected Behavior:** When creating a heap with a URI that doesn't exist, it should be created successfully.

**Actual Behavior:** The server reports that the heap already exists even when it doesn't.

**Root Cause Analysis:**
The issue is in the URI parsing logic in `heap_storage.rs`. When parsing `file://my-demo-heap.snapshot`:
1. The URI parser extracts the path as `/my-demo-heap.snapshot`
2. The `exists()` check looks for this path on the filesystem
3. If `/my-demo-heap.snapshot` exists as a directory (created by a previous failed operation or misconfiguration), it incorrectly reports the heap exists

### Bug 2: "Is a directory" Error When Saving Heap

**Production Error:**
```json
{
  "heap": "file://my-demo-heap.snapshot",
  "output": "Error saving heap: Is a directory (os error 21)"
}
```

**Expected Behavior:** After creating a heap and running JavaScript code, the heap state should be saved successfully.

**Actual Behavior:** When trying to save the heap, the operation fails with "Is a directory (os error 21)".

**Root Cause Analysis:**
This error occurs in `heap_storage.rs:39` in the `FileHeapStorage::put()` method:
```rust
async fn put(&self, name: &str, data: &[u8]) -> Result<(), String> {
    let path = self.dir.join(name);
    std::fs::write(path, data).map_err(|e| e.to_string())
}
```

The problem happens when:
1. The path extracted from the URI points to a directory instead of a file
2. `std::fs::write()` attempts to write to this path
3. The OS returns error 21 (EISDIR - "Is a directory")

## Test Cases

### 1. `test_create_heap_already_exists_false_positive()`
Reproduces the "heap already exists" false positive by:
- Creating a fresh temporary directory
- Attempting to create a heap with URI `file://my-demo-heap.snapshot`
- Verifying that the creation succeeds (should not report "already exists")

### 2. `test_heap_is_directory_error()`
Reproduces the "Is a directory" error by:
- Creating a heap successfully
- Loading the heap snapshot
- Executing JavaScript code
- Attempting to save the updated heap state
- Verifying that the save operation succeeds (should not fail with "Is a directory")

### 3. `test_uri_parsing_edge_cases()`
Tests various URI formats to identify parsing issues:
- `file://simple`
- `file://with-dash.ext`
- `file://my-demo-heap.snapshot` (production case)
- `file:///absolute/path/heap`

### 4. `test_production_scenario()`
Exactly replicates the production scenario:
1. Create heap with `file://my-demo-heap.snapshot`
2. Use the heap with the `run_js` tool
3. Execute JavaScript: `const myData = { counter: 1, message: "Hello from the heap!" }; myData;`
4. Save the heap state
5. Verify all operations succeed

### 5. `test_heap_file_not_directory()`
Verifies the filesystem behavior:
- Creates a heap
- Checks the actual file on disk
- Asserts that the heap is stored as a FILE, not a DIRECTORY

## Running the Tests

```bash
# Run all heap persistence tests
cargo test --test heap_persistence_integration

# Run with output
cargo test --test heap_persistence_integration -- --nocapture

# Run a specific test
cargo test --test heap_persistence_integration test_production_scenario -- --nocapture
```

## Expected Test Results

If the bugs are present, you will see:
- **PANIC**: "BUG REPRODUCED: Heap reported as existing when it shouldn't!"
- **PANIC**: "BUG REPRODUCED: Is a directory error: ..."
- **PANIC**: "BUG: Heap should be a FILE, not a directory!"

If the bugs are fixed, all tests should pass.

## Potential Fixes

### Fix for Bug 1 (Heap Already Exists):
The issue is likely in `MultiHeapStorage::parse_uri()` or `FileHeapStorage::exists()`. Consider:
1. Checking if the path is a file vs directory before reporting existence
2. Normalizing URIs to ensure consistent path handling
3. Using the configured heap directory properly instead of absolute paths

### Fix for Bug 2 (Is a Directory):
The issue is in how URIs are converted to filesystem paths. Consider:
1. Ensuring URIs like `file://my-heap` are stored as `<heap_dir>/my-heap` (file) not `<heap_dir>/my-heap/` (directory)
2. Adding validation to prevent directory creation
3. Checking path type before attempting write operations

## Implementation Details

### Test Helper Functions

#### `create_fresh_heap_dir()`
Creates a unique temporary directory for each test to ensure isolation:
```rust
fn create_fresh_heap_dir() -> PathBuf {
    let temp_dir = std::env::temp_dir();
    let unique_name = format!("heap-test-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0));
    let heap_dir = temp_dir.join(unique_name);
    // ... create directory ...
    heap_dir
}
```

### Dependencies Added

- `chrono = "0.4"` (dev-dependency) - for unique timestamp generation

### Module Exports

Added `src/lib.rs` to expose internal modules for integration testing:
```rust
pub mod mcp;
pub use mcp::initialize_v8;
pub use mcp::execute_stateful_for_test;
```

## Related Files

- `server/src/mcp.rs` - Main MCP server implementation with heap operations
- `server/src/mcp/heap_storage.rs` - Heap storage backends (File, S3, Multi)
- `server/src/main.rs` - Server startup and configuration

## Contact

If you encounter issues running these tests or have questions about the bugs, please refer to the production logs or contact the development team.
