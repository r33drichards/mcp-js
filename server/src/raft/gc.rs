use std::collections::HashSet;
use std::time::Duration;

use crate::mcp::heap_storage::{HeapStorage, AnyHeapStorage};
use crate::raft::store::SledStore;

/// Run a single GC sweep: delete staged files not referenced by committed state.
pub async fn gc_sweep(storage: &AnyHeapStorage, store: &SledStore) -> Result<(), String> {
    let state = store.get_state().await;

    // Collect all committed keys
    let committed_keys: HashSet<String> = state
        .sessions
        .values()
        .map(|r| r.key.clone())
        .collect();

    // List staged files and delete any not referenced
    let staged = storage.list_staged().await?;
    for key in staged {
        if !committed_keys.contains(&key) {
            let _ = storage.delete(&key).await;
        }
    }

    Ok(())
}

/// Spawn a background GC task that periodically cleans up uncommitted staged files.
pub fn spawn_gc_task(
    storage: AnyHeapStorage,
    store: SledStore,
    interval: Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(interval).await;
            if let Err(e) = gc_sweep(&storage, &store).await {
                tracing::warn!("GC sweep failed: {}", e);
            }
        }
    })
}
