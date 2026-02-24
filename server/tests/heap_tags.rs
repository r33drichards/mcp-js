use server::engine::heap_tags::HeapTagStore;
use std::collections::HashMap;

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
