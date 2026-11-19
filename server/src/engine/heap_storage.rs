use aws_sdk_s3::Client as S3Client;
use aws_sdk_s3::primitives::ByteStream;
use std::path::PathBuf;

use async_trait::async_trait;
use aws_config;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

/// Compatibility name for the shared blob backend used by heaps, filesystems and runtime payloads.
pub use mcp_metadata_store::blob::BlobStorage as HeapStorage;

/// In-memory blob backend. Primarily for tests and the `FsStore::in_memory`
/// constructor, but kept in the normal build so integration test crates (which
/// compile the lib without `--cfg test`) can use it.
#[derive(Clone, Default)]
pub struct MemoryHeapStorage {
    map: Arc<std::sync::Mutex<std::collections::HashMap<String, Vec<u8>>>>,
}

impl MemoryHeapStorage {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl HeapStorage for MemoryHeapStorage {
    async fn put_immutable(&self, name: &str, data: &[u8]) -> Result<(), String> {
        let mut map = self.map.lock().unwrap();
        match map.get(name) {
            Some(existing) if existing != data => Err("immutable object conflict".into()),
            Some(_) => Ok(()),
            None => {
                map.insert(name.into(), data.to_vec());
                Ok(())
            }
        }
    }

    async fn put(&self, name: &str, data: &[u8]) -> Result<(), String> {
        self.map
            .lock()
            .unwrap()
            .insert(name.to_string(), data.to_vec());
        Ok(())
    }
    async fn get(&self, name: &str) -> Result<Vec<u8>, String> {
        self.map
            .lock()
            .unwrap()
            .get(name)
            .cloned()
            .ok_or_else(|| format!("not found: {name}"))
    }
    async fn list(&self) -> Result<Vec<String>, String> {
        Ok(self.map.lock().unwrap().keys().cloned().collect())
    }
    async fn delete(&self, name: &str) -> Result<(), String> {
        self.map.lock().unwrap().remove(name);
        Ok(())
    }
    async fn contains(&self, name: &str) -> Result<bool, String> {
        Ok(self.map.lock().unwrap().contains_key(name))
    }
    async fn warm(&self, _name: &str) -> Result<(), String> {
        Ok(()) // already local
    }
}

#[derive(Clone)]
pub struct FileHeapStorage {
    dir: PathBuf,
    creation: Arc<Mutex<()>>,
    runtime_owner: Option<String>,
}

impl FileHeapStorage {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            creation: Arc::new(Mutex::new(())),
            runtime_owner: None,
        }
    }

    fn ensure_directory(path: &std::path::Path) -> Result<(), String> {
        match std::fs::create_dir(path) {
            Ok(()) => Self::sync_directory(
                path.parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or(std::path::Path::new(".")),
            ),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                if path.is_dir() {
                    Ok(())
                } else {
                    Err("blob parent is not a directory".into())
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let parent = path
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or(std::path::Path::new("."));
                Self::ensure_directory(parent)?;
                // Another writer may create it while we synchronize its parent.
                match std::fs::create_dir(path) {
                    Ok(()) => Self::sync_directory(parent),
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && path.is_dir() => {
                        Self::sync_directory(parent)
                    }
                    Err(e) => Err(e.to_string()),
                }
            }
            Err(e) => Err(e.to_string()),
        }
    }
    fn sync_directory(path: &std::path::Path) -> Result<(), String> {
        std::fs::File::open(path)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())
    }
    #[cfg(unix)]
    fn runtime_lock(&self, exclusive: bool) -> Result<std::fs::File, String> {
        use std::os::fd::AsRawFd;
        Self::ensure_directory(&self.dir)?;
        let path = self.dir.join(".runtime.lock");
        let file = match std::fs::File::open(&path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(path)
                .map_err(|e| e.to_string())?,
            Err(e) => return Err(e.to_string()),
        };
        let operation = if exclusive {
            libc::LOCK_EX
        } else {
            libc::LOCK_SH
        };
        if unsafe { libc::flock(file.as_raw_fd(), operation | libc::LOCK_NB) } != 0 {
            return Err(format!(
                "runtime storage maintenance is busy; retry: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(file)
    }
    fn mark_unmanaged_runtime(&self, name: &str) -> Result<(), String> {
        if self.runtime_owner.is_none()
            && name.starts_with("runtime/v2/")
            && !self.dir.join(".runtime-unmanaged").exists()
        {
            self.atomic_write(".runtime-unmanaged", b"unmanaged runtime objects", true)?;
        }
        Ok(())
    }
    fn runtime_files(&self) -> Result<Vec<(String, std::path::PathBuf)>, String> {
        fn visit(
            root: &std::path::Path,
            path: &std::path::Path,
            out: &mut Vec<(String, std::path::PathBuf)>,
        ) -> Result<(), String> {
            let metadata = match path.symlink_metadata() {
                Ok(metadata) => metadata,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(e) => return Err(e.to_string()),
            };
            if metadata.file_type().is_symlink() {
                return Err("runtime collection refuses symlinked object directories".into());
            }
            for entry in std::fs::read_dir(path).map_err(|e| e.to_string())? {
                let entry = entry.map_err(|e| e.to_string())?;
                let ty = entry.file_type().map_err(|e| e.to_string())?;
                if ty.is_dir() {
                    visit(root, &entry.path(), out)?
                } else if ty.is_file() {
                    out.push((
                        entry
                            .path()
                            .strip_prefix(root)
                            .map_err(|e| e.to_string())?
                            .to_str()
                            .ok_or("non-UTF8 runtime key")?
                            .replace('\\', "/"),
                        entry.path(),
                    ));
                }
            }
            Ok(())
        }
        let runtime = self.dir.join("runtime");
        match runtime.symlink_metadata() {
            Ok(m) if m.file_type().is_symlink() => {
                return Err("runtime collection refuses symlinked object directories".into());
            }
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.to_string()),
            _ => (),
        }
        let mut out = Vec::new();
        visit(&self.dir, &runtime.join("v2"), &mut out)?;
        Ok(out)
    }
    fn atomic_write(&self, name: &str, data: &[u8], immutable: bool) -> Result<(), String> {
        use std::io::Write;
        if name.is_empty()
            || std::path::Path::new(name)
                .components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            return Err("invalid blob key".into());
        }
        let path = self.dir.join(name);
        let parent = path.parent().ok_or("invalid blob parent")?;
        {
            // Root provisioning and newly created descendants must be durable
            // before a sibling writer can publish an object below them.
            let _creation = self
                .creation
                .lock()
                .map_err(|_| "blob directory lock poisoned")?;
            Self::ensure_directory(&self.dir)?;
            let mut directory = self.dir.clone();
            for component in parent
                .strip_prefix(&self.dir)
                .map_err(|e| e.to_string())?
                .components()
            {
                let child = directory.join(component);
                match std::fs::create_dir(&child) {
                    Ok(()) => (),
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && child.is_dir() => (),
                    Err(e) => return Err(e.to_string()),
                }
                // Sync within the configured root, also covering a directory
                // that a concurrent instance has just created. Never walk above
                // an existing root to reopen unrelated ancestors.
                Self::sync_directory(&directory)?;
                directory = child;
            }
        }
        let temp = parent.join(format!(".object-{}", uuid::Uuid::new_v4()));
        let result = (|| {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)
                .map_err(|e| e.to_string())?;
            file.write_all(data).map_err(|e| e.to_string())?;
            file.sync_all().map_err(|e| e.to_string())?;
            if immutable {
                match std::fs::hard_link(&temp, &path) {
                    Ok(()) => (),
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                        if std::fs::read(&path).map_err(|e| e.to_string())? != data {
                            return Err("immutable object conflict".into());
                        }
                        std::fs::File::open(&path)
                            .and_then(|f| f.sync_all())
                            .map_err(|e| e.to_string())?;
                    }
                    Err(e) => return Err(e.to_string()),
                }
            } else {
                std::fs::rename(&temp, &path).map_err(|e| e.to_string())?;
            }
            Self::sync_directory(parent)
        })();
        let _ = std::fs::remove_file(temp);
        result
    }
}

#[async_trait]
impl HeapStorage for FileHeapStorage {
    fn runtime_gc_identity(&self) -> Option<Vec<u8>> {
        #[cfg(unix)]
        {
            Some(
                self.dir
                    .canonicalize()
                    .unwrap_or_else(|_| self.dir.clone())
                    .as_os_str()
                    .as_encoded_bytes()
                    .to_vec(),
            )
        }
        #[cfg(not(unix))]
        {
            None
        }
    }
    fn runtime_scope(&self, owner: &str) -> Result<Arc<dyn HeapStorage>, String> {
        #[cfg(unix)]
        {
            let id = uuid::Uuid::parse_str(owner).map_err(|e| e.to_string())?;
            if id.to_string() != owner {
                return Err("invalid runtime owner".into());
            }
            let owner_key = format!(".runtime-owners/{owner}");
            if !self.dir.join(&owner_key).exists() {
                let _exclusive = self.runtime_lock(true)?;
                if !self.dir.join(".runtime-owners").exists()
                    && !self.dir.join(".runtime-unmanaged").exists()
                    && !self.runtime_files()?.is_empty()
                {
                    self.atomic_write(
                        ".runtime-unmanaged",
                        b"legacy runtime objects require explicit adoption",
                        true,
                    )?;
                }
                self.atomic_write(&owner_key, b"runtime metadata owner", true)?;
            }
            let mut scoped = self.clone();
            scoped.runtime_owner = Some(owner.into());
            Ok(Arc::new(scoped))
        }
        #[cfg(not(unix))]
        {
            let _ = owner;
            Err("local runtime collection requires filesystem locking".into())
        }
    }
    fn runtime_read_lease(&self) -> Result<Arc<dyn mcp_metadata_store::blob::BlobLease>, String> {
        #[cfg(unix)]
        {
            Ok(Arc::new(self.runtime_lock(false)?))
        }
        #[cfg(not(unix))]
        {
            Ok(Arc::new(()))
        }
    }
    fn collect_runtime_objects(
        &self,
        owners: &std::collections::BTreeSet<String>,
        minimum_age: std::time::Duration,
        roots: &mut dyn FnMut() -> Result<std::collections::BTreeSet<String>, String>,
    ) -> Result<usize, String> {
        #[cfg(unix)]
        {
            let _exclusive = self.runtime_lock(true)?;
            if self.dir.join(".runtime-unmanaged").exists() {
                return Err(
                    "runtime root contains unmanaged/legacy objects; collection is disabled".into(),
                );
            }
            let registered = std::fs::read_dir(self.dir.join(".runtime-owners"))
                .map_err(|e| e.to_string())?
                .map(|e| {
                    e.map_err(|e| e.to_string()).and_then(|e| {
                        e.file_name()
                            .into_string()
                            .map_err(|_| "invalid runtime owner".into())
                    })
                })
                .collect::<Result<std::collections::BTreeSet<_>, String>>()?;
            if &registered != owners {
                return Err(
                    "all registered metadata owners must participate in runtime collection".into(),
                );
            }
            let reachable = roots()?;
            let mut removed = 0;
            for (key, path) in self.runtime_files()? {
                let parts: Vec<_> = key.split('/').collect();
                if parts.len() != 4
                    || !matches!(parts[2], "artifact" | "console")
                    || parts[3].len() != 64
                    || !parts[3]
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                {
                    continue;
                }
                if reachable.contains(&key) {
                    continue;
                }
                let age = path
                    .metadata()
                    .and_then(|m| m.modified())
                    .map_err(|e| e.to_string())?
                    .elapsed();
                if age.is_ok_and(|age| age >= minimum_age) {
                    std::fs::remove_file(&path).map_err(|e| e.to_string())?;
                    Self::sync_directory(path.parent().ok_or("invalid runtime object parent")?)?;
                    removed += 1;
                }
            }
            Ok(removed)
        }
        #[cfg(not(unix))]
        {
            let _ = (owners, minimum_age, roots);
            Err("local runtime collection requires filesystem locking".into())
        }
    }

    async fn put_immutable(&self, name: &str, data: &[u8]) -> Result<(), String> {
        let _lease = if name.starts_with("runtime/v2/") {
            Some(self.runtime_read_lease()?)
        } else {
            None
        };
        self.mark_unmanaged_runtime(name)?;
        self.atomic_write(name, data, true)
    }
    async fn put(&self, name: &str, data: &[u8]) -> Result<(), String> {
        let _lease = if name.starts_with("runtime/v2/") {
            Some(self.runtime_read_lease()?)
        } else {
            None
        };
        self.mark_unmanaged_runtime(name)?;
        self.atomic_write(name, data, false)
    }
    async fn get(&self, name: &str) -> Result<Vec<u8>, String> {
        let _lease = if name.starts_with("runtime/v2/") {
            Some(self.runtime_read_lease()?)
        } else {
            None
        };
        let path = self.dir.join(name);
        std::fs::read(path).map_err(|e| e.to_string())
    }
    async fn list(&self) -> Result<Vec<String>, String> {
        let mut out = Vec::new();
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(e) => e,
            // A backend dir that was never written to has nothing to list.
            Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(e) => return Err(e.to_string()),
        };
        for entry in entries {
            let entry = entry.map_err(|e| e.to_string())?;
            if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                if let Some(name) = entry.file_name().to_str() {
                    if !matches!(name, ".runtime.lock" | ".runtime-unmanaged")
                        && !name.starts_with(".object-")
                    {
                        out.push(name.to_string());
                    }
                }
            }
        }
        Ok(out)
    }
    async fn delete(&self, name: &str) -> Result<(), String> {
        let path = self.dir.join(name);
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
    async fn contains(&self, name: &str) -> Result<bool, String> {
        Ok(self.dir.join(name).exists())
    }
    async fn warm(&self, _name: &str) -> Result<(), String> {
        Ok(()) // this backend IS the local layer
    }
}

#[derive(Clone)]
pub struct S3HeapStorage {
    bucket: String,
    client: Arc<S3Client>,
    // The runtime the S3 client (and its hyper connection pool / IO reactor)
    // was created on. S3 calls are dispatched here even when invoked from
    // another runtime — e.g. the isolate's current-thread runtime during an
    // fs-snapshot blob fetch. Awaiting the aws future directly on a different
    // runtime hangs because the connection's IO reactor lives on this one.
    runtime: tokio::runtime::Handle,
}

impl S3HeapStorage {
    pub async fn new(bucket: impl Into<String>) -> Self {
        let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .load()
            .await;
        let mut s3_builder = aws_sdk_s3::config::Builder::from(&config);

        // Point at an S3-compatible store (MinIO, etc.) when a custom endpoint
        // is configured. The modern SDK also honors AWS_ENDPOINT_URL on its own,
        // but set it explicitly so behavior is independent of loader specifics.
        if let Ok(endpoint) = std::env::var("AWS_ENDPOINT_URL") {
            if !endpoint.is_empty() {
                s3_builder = s3_builder.endpoint_url(endpoint);
            }
        }

        // Path-style addressing — required by most S3-compatible stores
        // (MinIO and friends serve `host/bucket/key`, not `bucket.host/key`).
        // Real AWS uses virtual-hosted style, so this stays off unless asked.
        let force_path_style = std::env::var("AWS_S3_FORCE_PATH_STYLE")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);
        if force_path_style {
            s3_builder = s3_builder.force_path_style(true);
        }

        Self::from_client(bucket, S3Client::from_conf(s3_builder.build()))
    }

    /// Explicit clients support embedders and isolated endpoint contract tests.
    pub fn from_client(bucket: impl Into<String>, client: S3Client) -> Self {
        Self {
            bucket: bucket.into(),
            client: Arc::new(client),
            runtime: tokio::runtime::Handle::current(),
        }
    }

    async fn on_runtime<T: Send + 'static>(
        &self,
        future: impl std::future::Future<Output = Result<T, String>> + Send + 'static,
    ) -> Result<T, String> {
        struct CancelOnDrop(tokio::task::AbortHandle);
        impl Drop for CancelOnDrop {
            fn drop(&mut self) {
                self.0.abort();
            }
        }
        let task = self.runtime.spawn(future);
        let _cancel = CancelOnDrop(task.abort_handle());
        task.await.map_err(|e| format!("S3 task failed: {e}"))?
    }

    async fn put_blocking(&self, name: &str, data: &[u8]) -> Result<(), String> {
        let client = self.client.clone();
        let bucket = self.bucket.clone();
        let name = name.to_string();
        let data = data.to_vec();
        // Run on the S3 client's own runtime (see `runtime` field), awaiting the
        // JoinHandle — which is safe to poll from any runtime.
        self.on_runtime(async move {
            client
                .put_object()
                .bucket(bucket)
                .key(name)
                .body(ByteStream::from(data))
                .send()
                .await
                .map(|_| ())
                .map_err(|e| e.to_string())
        })
        .await
    }

    async fn get_blocking(&self, name: &str) -> Result<Vec<u8>, String> {
        let client = self.client.clone();
        let bucket = self.bucket.clone();
        let name = name.to_string();
        self.on_runtime(async move {
            let output = client
                .get_object()
                .bucket(bucket)
                .key(name)
                .send()
                .await
                .map_err(|e| e.to_string())?;
            let data = output.body.collect().await.map_err(|e| e.to_string())?;
            Ok::<Vec<u8>, String>(data.into_bytes().to_vec())
        })
        .await
    }
}

#[async_trait]
impl HeapStorage for S3HeapStorage {
    async fn put_immutable(&self, name: &str, data: &[u8]) -> Result<(), String> {
        let client = self.client.clone();
        let bucket = self.bucket.clone();
        let name = name.to_owned();
        let data = data.to_vec();
        self.on_runtime(async move {
            match client
                .put_object()
                .bucket(&bucket)
                .key(&name)
                .if_none_match("*")
                .body(ByteStream::from(data.clone()))
                .send()
                .await
            {
                Ok(_) => Ok(()),
                Err(error)
                    if error
                        .raw_response()
                        .is_some_and(|r| r.status().as_u16() == 412) =>
                {
                    let existing = client
                        .get_object()
                        .bucket(&bucket)
                        .key(&name)
                        .send()
                        .await
                        .map_err(|e| format!("immutable object verification: {e}"))?
                        .body
                        .collect()
                        .await
                        .map_err(|e| e.to_string())?
                        .into_bytes();
                    if existing.as_ref() == data.as_slice() {
                        Ok(())
                    } else {
                        Err("immutable object conflict".into())
                    }
                }
                Err(e) => Err(format!("immutable S3 write: {e}")),
            }
        })
        .await
    }

    async fn put(&self, name: &str, data: &[u8]) -> Result<(), String> {
        self.put_blocking(name, data).await
    }
    async fn get(&self, name: &str) -> Result<Vec<u8>, String> {
        self.get_blocking(name).await
    }
}

/// Default local-cache capacity (8 GiB). Large enough that small/medium working
/// sets stay fully cached, bounded enough that a multi-TB primary never tries to
/// mirror itself onto local disk.
pub const DEFAULT_CACHE_CAPACITY_BYTES: u64 = 8 * 1024 * 1024 * 1024;

/// Size-bounded record of what currently lives in the local cache. Eviction is
/// FIFO by insertion (a simple, allocation-free approximation of LRU); the
/// primary remains the source of truth, so an evicted blob is simply re-fetched
/// and re-cached on the next `get`.
#[derive(Default)]
struct CacheBound {
    sizes: HashMap<String, u64>,
    order: VecDeque<String>,
    total: u64,
    cap: u64,
}

impl CacheBound {
    /// Record `name` at `size` bytes and return the keys that must be evicted
    /// from the local cache to stay within `cap`.
    fn record(&mut self, name: &str, size: u64) -> Vec<String> {
        if let Some(old) = self.sizes.insert(name.to_string(), size) {
            self.total -= old;
        } else {
            self.order.push_back(name.to_string());
        }
        self.total += size;

        let mut evicted = Vec::new();
        while self.total > self.cap {
            // Never evict the entry we just recorded (it is at the back).
            if self.order.len() <= 1 {
                break;
            }
            let Some(victim) = self.order.pop_front() else {
                break;
            };
            if let Some(sz) = self.sizes.remove(&victim) {
                self.total -= sz;
            }
            evicted.push(victim);
        }
        evicted
    }

    fn forget(&mut self, name: &str) {
        if let Some(sz) = self.sizes.remove(name) {
            self.total -= sz;
            if let Some(pos) = self.order.iter().position(|k| k == name) {
                self.order.remove(pos);
            }
        }
    }
}

/// Write-through cache that wraps any primary HeapStorage with a **size-bounded**
/// local filesystem cache.
/// On put: writes to both the local FS cache and the primary storage.
/// On get: checks the FS cache first; falls back to the primary and caches the
/// result locally. When the cache exceeds its capacity, the oldest entries are
/// evicted from local disk (the primary is untouched, so reads still succeed).
#[derive(Clone)]
pub struct WriteThroughCacheHeapStorage<P: HeapStorage + Clone> {
    primary: P,
    cache: FileHeapStorage,
    bound: Arc<Mutex<CacheBound>>,
}

impl<P: HeapStorage + Clone> WriteThroughCacheHeapStorage<P> {
    pub fn new(primary: P, cache_dir: impl Into<PathBuf>) -> Self {
        Self::with_capacity_bytes(primary, cache_dir, DEFAULT_CACHE_CAPACITY_BYTES)
    }

    /// Build a write-through cache capped at `cap_bytes` of resident local data.
    pub fn with_capacity_bytes(primary: P, cache_dir: impl Into<PathBuf>, cap_bytes: u64) -> Self {
        Self {
            primary,
            cache: FileHeapStorage::new(cache_dir),
            bound: Arc::new(Mutex::new(CacheBound {
                cap: cap_bytes.max(1),
                ..Default::default()
            })),
        }
    }

    /// Record a freshly-cached blob and evict the oldest entries (from the local
    /// cache only) to stay within capacity. Eviction is best-effort.
    async fn note_cached(&self, name: &str, size: u64) {
        let evicted = self.bound.lock().unwrap().record(name, size);
        for key in evicted {
            let _ = self.cache.delete(&key).await;
        }
    }
}

#[async_trait]
impl<P: HeapStorage + Clone> HeapStorage for WriteThroughCacheHeapStorage<P> {
    async fn put_immutable(&self, name: &str, data: &[u8]) -> Result<(), String> {
        self.primary.put_immutable(name, data).await?;
        // The primary is durable; an unavailable cache must not invalidate it.
        if self.cache.put(name, data).await.is_ok() {
            self.note_cached(name, data.len() as u64).await;
        }
        Ok(())
    }

    async fn put(&self, name: &str, data: &[u8]) -> Result<(), String> {
        // Never let a failed authoritative write become a cache hit.
        self.primary.put(name, data).await?;
        self.cache.put(name, data).await?;
        self.note_cached(name, data.len() as u64).await;
        Ok(())
    }
    async fn get(&self, name: &str) -> Result<Vec<u8>, String> {
        // Check FS cache first
        if let Ok(data) = self.cache.get(name).await {
            return Ok(data);
        }
        // Cache miss: fetch from primary and populate cache
        let data = self.primary.get(name).await?;
        // Best-effort cache population; don't fail if local write fails
        if let Err(e) = self.cache.put(name, &data).await {
            tracing::warn!("Failed to populate FS cache for {}: {}", name, e);
        } else {
            self.note_cached(name, data.len() as u64).await;
        }
        Ok(data)
    }
    async fn list(&self) -> Result<Vec<String>, String> {
        self.primary.list().await
    }
    async fn delete(&self, name: &str) -> Result<(), String> {
        // Remove from both layers; the cache delete is best-effort.
        self.bound.lock().unwrap().forget(name);
        let _ = self.cache.delete(name).await;
        self.primary.delete(name).await
    }
    async fn contains(&self, name: &str) -> Result<bool, String> {
        self.cache.contains(name).await
    }
    async fn warm(&self, name: &str) -> Result<(), String> {
        // Already cached locally — nothing to fetch.
        if self.cache.contains(name).await.unwrap_or(false) {
            return Ok(());
        }
        // Pull from the primary (e.g. S3) and populate the local cache, so a
        // later get() served from the cache never crosses to a remote backend.
        let data = self.primary.get(name).await?;
        if let Err(e) = self.cache.put(name, &data).await {
            tracing::warn!("Failed to warm FS cache for {}: {}", name, e);
        } else {
            self.note_cached(name, data.len() as u64).await;
        }
        Ok(())
    }
}

/// S3 with local filesystem write-through cache.
pub type S3WithFsCacheHeapStorage = WriteThroughCacheHeapStorage<S3HeapStorage>;

#[derive(Clone)]
pub enum AnyHeapStorage {
    File(FileHeapStorage),
    S3(S3HeapStorage),
    S3WithFsCache(S3WithFsCacheHeapStorage),
}

#[async_trait::async_trait]
impl HeapStorage for AnyHeapStorage {
    fn runtime_gc_identity(&self) -> Option<Vec<u8>> {
        match self {
            Self::File(inner) => inner.runtime_gc_identity(),
            _ => None,
        }
    }
    fn runtime_scope(&self, owner: &str) -> Result<Arc<dyn HeapStorage>, String> {
        match self {
            Self::File(inner) => inner.runtime_scope(owner),
            _ => Err("runtime collection unsupported".into()),
        }
    }

    async fn put_immutable(&self, name: &str, data: &[u8]) -> Result<(), String> {
        match self {
            Self::File(inner) => inner.put_immutable(name, data).await,
            Self::S3(inner) => inner.put_immutable(name, data).await,
            Self::S3WithFsCache(inner) => inner.put_immutable(name, data).await,
        }
    }

    async fn put(&self, name: &str, data: &[u8]) -> Result<(), String> {
        match self {
            AnyHeapStorage::File(inner) => inner.put(name, data).await,
            AnyHeapStorage::S3(inner) => inner.put(name, data).await,
            AnyHeapStorage::S3WithFsCache(inner) => inner.put(name, data).await,
        }
    }
    async fn get(&self, name: &str) -> Result<Vec<u8>, String> {
        match self {
            AnyHeapStorage::File(inner) => inner.get(name).await,
            AnyHeapStorage::S3(inner) => inner.get(name).await,
            AnyHeapStorage::S3WithFsCache(inner) => inner.get(name).await,
        }
    }
    async fn list(&self) -> Result<Vec<String>, String> {
        match self {
            AnyHeapStorage::File(inner) => inner.list().await,
            AnyHeapStorage::S3(inner) => inner.list().await,
            AnyHeapStorage::S3WithFsCache(inner) => inner.list().await,
        }
    }
    async fn delete(&self, name: &str) -> Result<(), String> {
        match self {
            AnyHeapStorage::File(inner) => inner.delete(name).await,
            AnyHeapStorage::S3(inner) => inner.delete(name).await,
            AnyHeapStorage::S3WithFsCache(inner) => inner.delete(name).await,
        }
    }
}
/// Compatibility adapter; new callers can use the shared blob backend directly.
pub struct RuntimeHeapStorage<T: HeapStorage>(pub T);
#[async_trait]
impl<T: HeapStorage> HeapStorage for RuntimeHeapStorage<T> {
    async fn put(&self, name: &str, data: &[u8]) -> Result<(), String> {
        self.0.put(name, data).await
    }
    async fn get(&self, name: &str) -> Result<Vec<u8>, String> {
        self.0.get(name).await
    }
    async fn put_immutable(&self, name: &str, data: &[u8]) -> Result<(), String> {
        self.0.put_immutable(name, data).await
    }
}

/// Shared construction used by all storage domains; callers retain their own
/// enablement and directory defaults. Creating an S3 client must happen on the
/// runtime that will perform its I/O.
pub enum BlobBackendConfig {
    Directory {
        path: PathBuf,
    },
    S3 {
        bucket: String,
        cache_dir: Option<PathBuf>,
    },
}
impl BlobBackendConfig {
    pub async fn build(self) -> AnyHeapStorage {
        match self {
            Self::Directory { path } => AnyHeapStorage::File(FileHeapStorage::new(path)),
            Self::S3 {
                bucket,
                cache_dir: Some(path),
            } => AnyHeapStorage::S3WithFsCache(WriteThroughCacheHeapStorage::new(
                S3HeapStorage::new(bucket).await,
                path,
            )),
            Self::S3 {
                bucket,
                cache_dir: None,
            } => AnyHeapStorage::S3(S3HeapStorage::new(bucket).await),
        }
    }
}
