use server::engine::heap_tags::HeapTagStore;
use std::collections::HashMap;
use std::sync::Arc;

fn temp_tag_store() -> HeapTagStore {
    HeapTagStore::from_config(sled::Config::new().temporary(true)).expect("failed to open temp sled")
}

#[tokio::test]
async fn test_set_and_get_tags() {
    let store = temp_tag_store();

    let mut tags = HashMap::new();
    tags.insert("env".to_string(), "production".to_string());
    tags.insert("version".to_string(), "1.0".to_string());

    store.set_tags("abc123", tags.clone()).await.unwrap();

    let result = store.get_tags("abc123").await.unwrap();
    assert_eq!(result, tags);
}

#[tokio::test]
async fn test_get_tags_nonexistent() {
    let store = temp_tag_store();

    let result = store.get_tags("nonexistent").await.unwrap();
    assert!(result.is_empty());
}

#[tokio::test]
async fn test_delete_all_tags() {
    let store = temp_tag_store();

    let mut tags = HashMap::new();
    tags.insert("env".to_string(), "production".to_string());
    store.set_tags("abc123", tags).await.unwrap();

    // Delete all tags
    store.delete_tags("abc123", None).await.unwrap();

    let result = store.get_tags("abc123").await.unwrap();
    assert!(result.is_empty());
}

#[tokio::test]
async fn test_delete_specific_keys() {
    let store = temp_tag_store();

    let mut tags = HashMap::new();
    tags.insert("env".to_string(), "production".to_string());
    tags.insert("version".to_string(), "1.0".to_string());
    tags.insert("team".to_string(), "backend".to_string());
    store.set_tags("abc123", tags).await.unwrap();

    // Delete only "env" and "team"
    store
        .delete_tags(
            "abc123",
            Some(vec!["env".to_string(), "team".to_string()]),
        )
        .await
        .unwrap();

    let result = store.get_tags("abc123").await.unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result.get("version").unwrap(), "1.0");
}

#[tokio::test]
async fn test_query_by_tags() {
    let store = temp_tag_store();

    let mut tags1 = HashMap::new();
    tags1.insert("env".to_string(), "production".to_string());
    tags1.insert("version".to_string(), "1.0".to_string());
    store.set_tags("heap_a", tags1).await.unwrap();

    let mut tags2 = HashMap::new();
    tags2.insert("env".to_string(), "staging".to_string());
    tags2.insert("version".to_string(), "1.0".to_string());
    store.set_tags("heap_b", tags2).await.unwrap();

    let mut tags3 = HashMap::new();
    tags3.insert("env".to_string(), "production".to_string());
    tags3.insert("version".to_string(), "2.0".to_string());
    store.set_tags("heap_c", tags3).await.unwrap();

    // Query for env=production
    let mut filter = HashMap::new();
    filter.insert("env".to_string(), "production".to_string());
    let results = store.query_by_tags(filter).await.unwrap();
    assert_eq!(results.len(), 2);
    let heaps: Vec<&str> = results.iter().map(|e| e.heap.as_str()).collect();
    assert!(heaps.contains(&"heap_a"));
    assert!(heaps.contains(&"heap_c"));

    // Query for env=production AND version=1.0
    let mut filter2 = HashMap::new();
    filter2.insert("env".to_string(), "production".to_string());
    filter2.insert("version".to_string(), "1.0".to_string());
    let results2 = store.query_by_tags(filter2).await.unwrap();
    assert_eq!(results2.len(), 1);
    assert_eq!(results2[0].heap, "heap_a");
}

#[tokio::test]
async fn test_query_no_match() {
    let store = temp_tag_store();

    let mut tags = HashMap::new();
    tags.insert("env".to_string(), "production".to_string());
    store.set_tags("heap_a", tags).await.unwrap();

    let mut filter = HashMap::new();
    filter.insert("env".to_string(), "development".to_string());
    let results = store.query_by_tags(filter).await.unwrap();
    assert!(results.is_empty());
}

#[tokio::test]
async fn test_overwrite_tags() {
    let store = temp_tag_store();

    let mut tags1 = HashMap::new();
    tags1.insert("env".to_string(), "production".to_string());
    tags1.insert("version".to_string(), "1.0".to_string());
    store.set_tags("abc123", tags1).await.unwrap();

    // Overwrite with new tags
    let mut tags2 = HashMap::new();
    tags2.insert("env".to_string(), "staging".to_string());
    store.set_tags("abc123", tags2.clone()).await.unwrap();

    let result = store.get_tags("abc123").await.unwrap();
    assert_eq!(result, tags2);
    assert!(!result.contains_key("version")); // old key should be gone
}

#[tokio::test]
async fn test_query_excludes_empty_tags() {
    let store = temp_tag_store();

    let mut tags = HashMap::new();
    tags.insert("env".to_string(), "production".to_string());
    store.set_tags("heap_a", tags).await.unwrap();

    // Set then delete tags on heap_b (tombstone)
    let mut tags2 = HashMap::new();
    tags2.insert("env".to_string(), "production".to_string());
    store.set_tags("heap_b", tags2).await.unwrap();
    store.delete_tags("heap_b", None).await.unwrap();

    // Query should only return heap_a
    let mut filter = HashMap::new();
    filter.insert("env".to_string(), "production".to_string());
    let results = store.query_by_tags(filter).await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].heap, "heap_a");
}

/// Two writers concurrently merge different keys into the same heap.
/// After both complete, the result must contain ALL keys from both writers.
#[tokio::test]
async fn test_concurrent_merge_different_keys() {
    let store = Arc::new(temp_tag_store());
    let heap = "concurrent_heap";

    // Spawn many pairs of concurrent writers to increase the chance of
    // exposing a race condition if the implementation is non-atomic.
    for _ in 0..50 {
        // Reset the heap to a clean state each iteration
        store.set_tags(heap, HashMap::new()).await.unwrap();

        let store_a = Arc::clone(&store);
        let store_b = Arc::clone(&store);

        let handle_a = tokio::spawn(async move {
            let mut tags = HashMap::new();
            tags.insert("foo".to_string(), "bar".to_string());
            store_a.merge_tags("concurrent_heap", tags).await.unwrap();
        });

        let handle_b = tokio::spawn(async move {
            let mut tags = HashMap::new();
            tags.insert("baz".to_string(), "blah".to_string());
            store_b.merge_tags("concurrent_heap", tags).await.unwrap();
        });

        handle_a.await.unwrap();
        handle_b.await.unwrap();

        let result = store.get_tags(heap).await.unwrap();
        assert_eq!(
            result.get("foo").map(|s| s.as_str()),
            Some("bar"),
            "Writer A's key 'foo' was lost in concurrent merge"
        );
        assert_eq!(
            result.get("baz").map(|s| s.as_str()),
            Some("blah"),
            "Writer B's key 'baz' was lost in concurrent merge"
        );
        assert_eq!(result.len(), 2, "Expected exactly 2 keys after concurrent merge");
    }
}

/// Many writers concurrently merge distinct keys into the same heap.
/// Verifies no keys are lost under higher contention.
#[tokio::test]
async fn test_concurrent_merge_many_writers() {
    let store = Arc::new(temp_tag_store());
    let heap = "many_writers_heap";
    let num_writers = 20;

    let mut handles = Vec::new();
    for i in 0..num_writers {
        let store_clone = Arc::clone(&store);
        let key = format!("key_{}", i);
        let value = format!("value_{}", i);
        handles.push(tokio::spawn(async move {
            let mut tags = HashMap::new();
            tags.insert(key, value);
            store_clone.merge_tags("many_writers_heap", tags).await.unwrap();
        }));
    }

    for handle in handles {
        handle.await.unwrap();
    }

    let result = store.get_tags(heap).await.unwrap();
    assert_eq!(
        result.len(),
        num_writers,
        "Expected {} keys but got {}. Some writes were lost!",
        num_writers,
        result.len()
    );
    for i in 0..num_writers {
        let key = format!("key_{}", i);
        let expected_value = format!("value_{}", i);
        assert_eq!(
            result.get(&key).map(|s| s.as_str()),
            Some(expected_value.as_str()),
            "Key '{}' was lost or corrupted",
            key
        );
    }
}

/// Two writers concurrently merge the SAME key with different values.
/// Both keys from each writer should survive; for the contested key,
/// one of the two values must win (last-writer-wins), but neither
/// writer's OTHER keys should be lost.
#[tokio::test]
async fn test_concurrent_merge_same_key_no_collateral_loss() {
    let store = Arc::new(temp_tag_store());
    let heap = "same_key_heap";

    for _ in 0..50 {
        store.set_tags(heap, HashMap::new()).await.unwrap();

        let store_a = Arc::clone(&store);
        let store_b = Arc::clone(&store);

        // Writer A: sets "shared" to "from_a" AND "only_a" to "a_val"
        let handle_a = tokio::spawn(async move {
            let mut tags = HashMap::new();
            tags.insert("shared".to_string(), "from_a".to_string());
            tags.insert("only_a".to_string(), "a_val".to_string());
            store_a.merge_tags("same_key_heap", tags).await.unwrap();
        });

        // Writer B: sets "shared" to "from_b" AND "only_b" to "b_val"
        let handle_b = tokio::spawn(async move {
            let mut tags = HashMap::new();
            tags.insert("shared".to_string(), "from_b".to_string());
            tags.insert("only_b".to_string(), "b_val".to_string());
            store_b.merge_tags("same_key_heap", tags).await.unwrap();
        });

        handle_a.await.unwrap();
        handle_b.await.unwrap();

        let result = store.get_tags(heap).await.unwrap();

        // The contested "shared" key must have one of the two values
        let shared_val = result.get("shared").expect("'shared' key must exist");
        assert!(
            shared_val == "from_a" || shared_val == "from_b",
            "Expected 'shared' to be 'from_a' or 'from_b', got '{}'",
            shared_val
        );

        // Non-contested keys must BOTH survive — no collateral data loss
        assert_eq!(
            result.get("only_a").map(|s| s.as_str()),
            Some("a_val"),
            "Writer A's non-contested key 'only_a' was lost"
        );
        assert_eq!(
            result.get("only_b").map(|s| s.as_str()),
            Some("b_val"),
            "Writer B's non-contested key 'only_b' was lost"
        );
        assert_eq!(result.len(), 3, "Expected exactly 3 keys (shared + only_a + only_b)");
    }
}
