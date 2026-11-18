//! Shared named-blob I/O. Object identity belongs to the content-addressed layer.
use async_trait::async_trait;

#[async_trait]
pub trait BlobStorage: Send + Sync + 'static {
    async fn put(&self, name: &str, data: &[u8]) -> Result<(), String>;
    async fn get(&self, name: &str) -> Result<Vec<u8>, String>;

    /// Create an immutable object or acknowledge an identical existing object.
    /// Implementations must never overwrite conflicting bytes. Unsupported is
    /// an error, not a fallback to mutable put.
    async fn put_immutable(&self, _name: &str, _data: &[u8]) -> Result<(), String> {
        Err("immutable object writes are unsupported by this backend".into())
    }

    /// List every blob name in the backend. Used by fs snapshot GC. Backends
    /// that cannot enumerate return an error (GC is then unavailable on them).
    async fn list(&self) -> Result<Vec<String>, String> {
        Err("list is not supported by this storage backend".to_string())
    }

    /// Delete a blob by name. Used by fs snapshot GC sweep.
    async fn delete(&self, _name: &str) -> Result<(), String> {
        Err("delete is not supported by this storage backend".to_string())
    }

    /// Whether a blob is present in this backend's *local* layer. Cheap by
    /// default only where it can be (a local file/memory store); the generic
    /// fallback actually fetches. Used by `warm` to skip already-local blobs.
    async fn contains(&self, name: &str) -> Result<bool, String> {
        Ok(self.get(name).await.is_ok())
    }

    /// Ensure a blob is materialized in this backend's local layer so a later
    /// `get` does not have to cross to a remote backend (or another tokio
    /// runtime). On a write-through cache this populates the local cache from
    /// the primary; on a purely-local backend it is a no-op. Pre-staging fs
    /// blobs via `warm` on the main runtime is what lets the isolate's
    /// current-thread ops read them without awaiting remote I/O.
    async fn warm(&self, name: &str) -> Result<(), String> {
        self.get(name).await.map(|_| ())
    }
}
