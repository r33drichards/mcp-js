/// Common test utilities
use std::fs;
use std::path::PathBuf;

/// Creates a temporary directory for heap storage
pub fn create_temp_heap_dir() -> String {
    let dir = format!("/tmp/mcp-v8-test-heaps-{}", uuid::Uuid::new_v4());
    fs::create_dir_all(&dir).expect("Failed to create temp heap directory");
    dir
}

/// Creates a temporary directory for resource storage
pub fn create_temp_resource_dir() -> String {
    let dir = format!("/tmp/mcp-v8-test-resources-{}", uuid::Uuid::new_v4());
    fs::create_dir_all(&dir).expect("Failed to create temp resource directory");
    dir
}

/// Cleans up a temporary heap directory
pub fn cleanup_heap_dir(dir: &str) {
    if let Err(e) = fs::remove_dir_all(dir) {
        eprintln!("Warning: Failed to cleanup heap dir {}: {}", dir, e);
    }
}

/// Cleans up a temporary resource directory
pub fn cleanup_resource_dir(dir: &str) {
    if let Err(e) = fs::remove_dir_all(dir) {
        eprintln!("Warning: Failed to cleanup resource dir {}: {}", dir, e);
    }
}
