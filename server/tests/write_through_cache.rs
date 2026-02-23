/// Tests for WriteThroughCacheHeapStorage.
///
/// Uses FileHeapStorage as the primary backend (standing in for S3)
/// so the write-through cache logic can be exercised without AWS credentials.

use server::engine::heap_storage::{FileHeapStorage, HeapStorage, WriteThroughCacheHeapStorage};
use std::path::PathBuf;

fn temp_dir(suffix: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mcp-wt-cache-test-{}-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("unknown"),
        suffix
    ));
    std::fs::create_dir_all(&dir).ok();
    dir
}

fn cleanup(dir: &PathBuf) {
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn test_put_writes_to_both_cache_and_primary() {
    let primary_dir = temp_dir("primary-put");
    let cache_dir = temp_dir("cache-put");

    let primary = FileHeapStorage::new(&primary_dir);
    let cached = WriteThroughCacheHeapStorage::new(primary, &cache_dir);

    let data = b"hello world";
    cached.put("key1", data).await.unwrap();

    // Verify data exists in both the cache and primary directories
    let cached_data = std::fs::read(cache_dir.join("key1")).unwrap();
    assert_eq!(cached_data, data);

    let primary_data = std::fs::read(primary_dir.join("key1")).unwrap();
    assert_eq!(primary_data, data);

    cleanup(&primary_dir);
    cleanup(&cache_dir);
}

#[tokio::test]
async fn test_get_returns_from_cache_on_hit() {
    let primary_dir = temp_dir("primary-cache-hit");
    let cache_dir = temp_dir("cache-cache-hit");

    let primary = FileHeapStorage::new(&primary_dir);
    let cached = WriteThroughCacheHeapStorage::new(primary, &cache_dir);

    // Write data via the cache (populates both)
    cached.put("key1", b"original").await.unwrap();

    // Now change the primary directly to prove the cache is used, not the primary
    std::fs::write(primary_dir.join("key1"), b"modified-in-primary").unwrap();

    let result = cached.get("key1").await.unwrap();
    assert_eq!(result, b"original", "Should return cached data, not primary");

    cleanup(&primary_dir);
    cleanup(&cache_dir);
}

#[tokio::test]
async fn test_get_falls_back_to_primary_on_cache_miss() {
    let primary_dir = temp_dir("primary-miss");
    let cache_dir = temp_dir("cache-miss");

    let primary = FileHeapStorage::new(&primary_dir);
    let cached = WriteThroughCacheHeapStorage::new(primary, &cache_dir);

    // Write data only to the primary directory (simulating data in S3 but not cached)
    std::fs::write(primary_dir.join("key1"), b"from-primary").unwrap();

    let result = cached.get("key1").await.unwrap();
    assert_eq!(result, b"from-primary", "Should fall back to primary on cache miss");

    cleanup(&primary_dir);
    cleanup(&cache_dir);
}

#[tokio::test]
async fn test_get_populates_cache_after_primary_fetch() {
    let primary_dir = temp_dir("primary-populate");
    let cache_dir = temp_dir("cache-populate");

    let primary = FileHeapStorage::new(&primary_dir);
    let cached = WriteThroughCacheHeapStorage::new(primary, &cache_dir);

    // Write data only to the primary (cache miss scenario)
    std::fs::write(primary_dir.join("key1"), b"from-primary").unwrap();

    // First get: cache miss, fetches from primary
    let result = cached.get("key1").await.unwrap();
    assert_eq!(result, b"from-primary");

    // Verify the cache was populated
    let cache_data = std::fs::read(cache_dir.join("key1")).unwrap();
    assert_eq!(cache_data, b"from-primary", "Cache should be populated after miss");

    // Second get: should now come from cache even if primary is changed
    std::fs::write(primary_dir.join("key1"), b"modified-in-primary").unwrap();
    let result2 = cached.get("key1").await.unwrap();
    assert_eq!(result2, b"from-primary", "Second get should use cached copy");

    cleanup(&primary_dir);
    cleanup(&cache_dir);
}

#[tokio::test]
async fn test_get_returns_error_when_both_miss() {
    let primary_dir = temp_dir("primary-both-miss");
    let cache_dir = temp_dir("cache-both-miss");

    let primary = FileHeapStorage::new(&primary_dir);
    let cached = WriteThroughCacheHeapStorage::new(primary, &cache_dir);

    let result = cached.get("nonexistent").await;
    assert!(result.is_err(), "Should return error when key is in neither cache nor primary");

    cleanup(&primary_dir);
    cleanup(&cache_dir);
}

#[tokio::test]
async fn test_multiple_keys_isolated() {
    let primary_dir = temp_dir("primary-multi");
    let cache_dir = temp_dir("cache-multi");

    let primary = FileHeapStorage::new(&primary_dir);
    let cached = WriteThroughCacheHeapStorage::new(primary, &cache_dir);

    cached.put("key1", b"value1").await.unwrap();
    cached.put("key2", b"value2").await.unwrap();

    assert_eq!(cached.get("key1").await.unwrap(), b"value1");
    assert_eq!(cached.get("key2").await.unwrap(), b"value2");

    // Keys don't interfere with each other
    assert_ne!(cached.get("key1").await.unwrap(), cached.get("key2").await.unwrap());

    cleanup(&primary_dir);
    cleanup(&cache_dir);
}

#[tokio::test]
async fn test_put_overwrites_existing_data() {
    let primary_dir = temp_dir("primary-overwrite");
    let cache_dir = temp_dir("cache-overwrite");

    let primary = FileHeapStorage::new(&primary_dir);
    let cached = WriteThroughCacheHeapStorage::new(primary, &cache_dir);

    cached.put("key1", b"first").await.unwrap();
    assert_eq!(cached.get("key1").await.unwrap(), b"first");

    cached.put("key1", b"second").await.unwrap();
    assert_eq!(cached.get("key1").await.unwrap(), b"second");

    // Both locations updated
    assert_eq!(std::fs::read(cache_dir.join("key1")).unwrap(), b"second");
    assert_eq!(std::fs::read(primary_dir.join("key1")).unwrap(), b"second");

    cleanup(&primary_dir);
    cleanup(&cache_dir);
}

#[tokio::test]
async fn test_large_binary_data() {
    let primary_dir = temp_dir("primary-large");
    let cache_dir = temp_dir("cache-large");

    let primary = FileHeapStorage::new(&primary_dir);
    let cached = WriteThroughCacheHeapStorage::new(primary, &cache_dir);

    // Simulate a realistic heap snapshot size (512KB)
    let data: Vec<u8> = (0..512 * 1024).map(|i| (i % 256) as u8).collect();

    cached.put("big-heap", &data).await.unwrap();

    let result = cached.get("big-heap").await.unwrap();
    assert_eq!(result, data, "Large binary data should round-trip correctly");

    cleanup(&primary_dir);
    cleanup(&cache_dir);
}
