//! The engine's FFI surface: uniffi-exported records, error type, factory
//! functions, and the exported `impl Engine` block, plus the builder used by
//! Rust hosts. A child module of `engine`, so it works on the engine's own
//! fields directly — there is no wrapper type and no delegation layer.
//!
//! Everything here is re-exported from `crate::engine`.

use super::*;
use super::ffi_config::{BlobStore, EngineConfig, ExecutionLimits, FilesystemAccess, StoreBackend};
use super::fs_mount::SessionMount;
use super::session_log::SessionLogEntry;
use crate::bootstrap::{
    build_storage_engine, CapabilityBootstrapConfig, FeatureBootstrapConfig, PolicyBootstrapConfig,
    RuntimeBootstrap, StorageBootstrapConfig,
};
use crate::cli::StoreKind;

/// Runtime clones can be released on a Tokio worker when an execution ends.
/// Nonblocking teardown avoids Tokio's panic when the last owner drops there.
pub(super) struct OwnedRuntime(Option<tokio::runtime::Runtime>);

impl std::ops::Deref for OwnedRuntime {
    type Target = tokio::runtime::Runtime;
    fn deref(&self) -> &Self::Target {
        self.0.as_ref().expect("runtime exists until drop")
    }
}

impl Drop for OwnedRuntime {
    fn drop(&mut self) {
        if let Some(runtime) = self.0.take() {
            runtime.shutdown_background();
        }
    }
}

pub const DEFAULT_WASM_STUB_PREFIX: &str = crate::engine::wasm_stub::DEFAULT_WASM_STUB_PREFIX;
pub const DEFAULT_MCP_STUB_PREFIX: &str = crate::engine::mcp_client::DEFAULT_STUB_PREFIX;
#[derive(Clone, Copy, Debug, uniffi::Enum)]
pub enum RuntimeMode {
    Stateless,
    LocalStateful,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum RuntimeLifecycleState {
    Running,
    ShuttingDown,
    Shutdown,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct RuntimeShutdownResult {
    pub cancelled_executions: u64,
    pub closed_mcp_connections: u64,
    pub cluster_shutdown: bool,
    pub already_shutdown: bool,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct ToolDefinition {
    pub name: String,
    pub description: Option<String>,
    pub input_schema_json: String,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct McpRequestHeaders {
    pub values: HashMap<String, String>,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct ToolCallRequest {
    pub name: String,
    pub arguments_json: String,
    pub session_id: Option<String>,
    pub mcp_headers: Option<McpRequestHeaders>,
}

#[derive(Clone, Debug, Serialize, uniffi::Record)]
pub struct RuntimeCapabilities {
    pub heap: bool,
    pub filesystem: bool,
    pub sessions: bool,
}

/// Classification of a native filesystem failure. Mirrors [`fs::FsErrorKind`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum FsErrorKind {
    NotFound,
    PermissionDenied,
    AlreadyExists,
    NotDirectory,
    IsDirectory,
    NotEmpty,
    InvalidData,
    NotSupported,
    Other,
}

impl From<fs::FsErrorKind> for FsErrorKind {
    fn from(kind: fs::FsErrorKind) -> Self {
        match kind {
            fs::FsErrorKind::NotFound => Self::NotFound,
            fs::FsErrorKind::PermissionDenied => Self::PermissionDenied,
            fs::FsErrorKind::AlreadyExists => Self::AlreadyExists,
            fs::FsErrorKind::NotDirectory => Self::NotDirectory,
            fs::FsErrorKind::IsDirectory => Self::IsDirectory,
            fs::FsErrorKind::NotEmpty => Self::NotEmpty,
            fs::FsErrorKind::InvalidData => Self::InvalidData,
            fs::FsErrorKind::NotSupported => Self::NotSupported,
            fs::FsErrorKind::Other => Self::Other,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum FsEntryKind {
    File,
    Directory,
    Symlink,
    Other,
}

/// Metadata returned by the native `fs_stat` / `fs_lstat` calls.
#[derive(Clone, Debug, uniffi::Record)]
pub struct FsMetadata {
    pub kind: FsEntryKind,
    pub size: u64,
    pub readonly: bool,
    /// Unix mode bits (type bits included); synthesized on other platforms.
    pub mode: u32,
    /// Modification time in milliseconds since the Unix epoch, when known.
    pub modified_ms: Option<f64>,
}

impl From<fs::FsStat> for FsMetadata {
    fn from(stat: fs::FsStat) -> Self {
        let kind = if stat.is_symlink {
            FsEntryKind::Symlink
        } else if stat.is_directory {
            FsEntryKind::Directory
        } else if stat.is_file {
            FsEntryKind::File
        } else {
            FsEntryKind::Other
        };
        Self {
            kind,
            size: stat.size,
            readonly: stat.readonly,
            mode: stat.mode,
            modified_ms: stat.mtime_ms,
        }
    }
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct ExecutionRequest {
    pub code: String,
    pub file: Option<String>,
    pub heap: Option<String>,
    pub fs: Option<String>,
    pub session: Option<String>,
    pub heap_memory_max_mb: Option<u64>,
    pub execution_timeout_secs: Option<u64>,
    pub tags: Option<HashMap<String, String>>,
    pub mcp_headers: Option<McpRequestHeaders>,
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum RuntimeError {
    #[error("invalid configuration: {message}")]
    InvalidConfig { message: String },
    #[error("failed to initialize the embedded runtime: {message}")]
    Initialization { message: String },
    #[error("invalid JSON for {field}: {message}")]
    InvalidJson { field: String, message: String },
    #[error("tool call failed: {message}")]
    ToolCall { message: String },
    #[error("operation failed: {message}")]
    Operation { message: String },
    /// A native filesystem call failed. `message` is the same text the guest
    /// `fs.*` wrapper would surface for the failure.
    #[error("filesystem operation failed: {message}")]
    FileSystem { kind: FsErrorKind, message: String },
    /// A configuration builder's `build()` found a required field unset.
    #[error("{message}")]
    MissingRequiredField {
        record_type: String,
        field: String,
        message: String,
    },
}

impl RuntimeError {
    /// The error a `<Record>Builder::build()` reports for an unset required field.
    pub fn missing(record_type: &str, field: &str) -> Self {
        Self::MissingRequiredField {
            record_type: record_type.to_string(),
            field: field.to_string(),
            message: format!("{record_type} is missing required field {field}"),
        }
    }
}

impl From<fs::FsError> for RuntimeError {
    fn from(error: fs::FsError) -> Self {
        Self::FileSystem { kind: error.kind.into(), message: error.message }
    }
}

impl RuntimeError {
    /// The bare error message, without the variant's display prefix. Transport
    /// layers use this to keep wire-visible error strings identical to the
    /// underlying operation error.
    pub fn message(&self) -> &str {
        match self {
            Self::InvalidConfig { message }
            | Self::Initialization { message }
            | Self::ToolCall { message }
            | Self::Operation { message } => message,
            Self::InvalidJson { message, .. }
            | Self::FileSystem { message, .. }
            | Self::MissingRequiredField { message, .. } => message,
        }
    }
}

/// A blob store record resolved into the bootstrap's per-axis fields.
struct BlobStoreParts {
    kind: StoreKind,
    dir: Option<String>,
    /// `(bucket, cache_dir)` for S3 stores.
    s3: Option<(Option<String>, Option<String>)>,
}

fn blob_store_parts(store: Option<&BlobStore>, default_dir: &str) -> Result<BlobStoreParts, RuntimeError> {
    let Some(store) = store else {
        return Ok(BlobStoreParts { kind: StoreKind::None, dir: None, s3: None });
    };
    match store.backend {
        StoreBackend::Directory => Ok(BlobStoreParts {
            kind: StoreKind::Dir,
            dir: Some(store.path.clone().unwrap_or_else(|| default_dir.to_string())),
            s3: None,
        }),
        StoreBackend::S3 => {
            let bucket = store.bucket.clone().filter(|b| !b.is_empty()).ok_or_else(|| {
                RuntimeError::InvalidConfig {
                    message: "an S3 blob store requires a bucket".into(),
                }
            })?;
            Ok(BlobStoreParts {
                kind: StoreKind::S3,
                dir: None,
                s3: Some((Some(bucket), store.path.clone())),
            })
        }
    }
}

#[uniffi::export]
impl Engine {
    /// Enable hook/policy-gated host filesystem access without enabling
    /// subprocesses. Equivalent to `create` with only `limits` and
    /// `filesystem` set; `filesystem_policy_json` is `FilesystemAccess::policies_json`.
    #[uniffi::constructor]
    pub fn create_with_filesystem(
        heap_memory_max_mb: u64,
        execution_timeout_secs: u64,
        filesystem_policy_json: String,
    ) -> Result<Arc<Self>, RuntimeError> {
        Self::create(EngineConfig {
            limits: ExecutionLimits {
                heap_memory_max_mb,
                execution_timeout_secs,
                max_concurrent_executions: None,
            },
            data_dir: None,
            filesystem: Some(FilesystemAccess {
                policies_json: filesystem_policy_json,
                passthrough: None,
            }),
            heap_store: None,
            fs_snapshot_store: None,
            fs_labels_db: None,
            wasm_modules: None,
        })
    }

    /// True when this engine was constructed with a filesystem hook chain, so
    /// guest `fs.*` calls and native file views are available.
    pub fn host_filesystem_enabled(&self) -> bool {
        self.fs_config.is_some()
    }

    /// A native file view. `None` is the host filesystem behind the engine's
    /// hook chain. `Some(session)` is that session's filesystem snapshot: the
    /// same snapshot `run_js` mounts for the session, so native writes are
    /// visible to the next run and guest writes to the next native read. Each
    /// mutating call folds the change into a new snapshot recorded in the
    /// session log, exactly as a run does.
    pub fn fs_view(self: Arc<Self>, session: Option<String>) -> Result<Arc<FsView>, RuntimeError> {
        if self.fs_config.is_none() {
            return Err(RuntimeError::Operation {
                message: "filesystem access is not configured; set EngineConfig.filesystem".into(),
            });
        }
        if let Some(session) = &session {
            if session.is_empty() {
                return Err(RuntimeError::InvalidConfig {
                    message: "session name must not be empty".into(),
                });
            }
            if self.fs_store.is_none() || self.session_log.is_none() {
                return Err(RuntimeError::Operation {
                    message: "session file views require filesystem snapshots; set EngineConfig.fs_snapshot_store".into(),
                });
            }
        }
        Ok(Arc::new(FsView {
            engine: self,
            session,
        }))
    }

    /// Wait for an execution to reach a terminal status and return its record,
    /// including the heap and filesystem snapshot ids it produced.
    pub async fn await_execution(
        self: Arc<Self>,
        execution_id: String,
    ) -> Result<ExecutionInfo, RuntimeError> {
        let engine = self.clone();
        self.on_runtime(async move {
            loop {
                let info = engine.get_execution(execution_id.clone())?;
                match info.status.as_str() {
                    "pending" | "running" => {
                        tokio::time::sleep(std::time::Duration::from_millis(25)).await
                    }
                    _ => return Ok(info),
                }
            }
        })
        .await
    }

    /// Read a file on the host filesystem; see `fs_view(None)`.
    pub async fn fs_read_file(self: Arc<Self>, path: String) -> Result<Vec<u8>, RuntimeError> {
        self.fs_view(None)?.read_file(path).await
    }

    /// Read at most `max_bytes` bytes of a host file starting at `offset`.
    pub async fn fs_read_file_range(
        self: Arc<Self>,
        path: String,
        offset: u64,
        max_bytes: u64,
    ) -> Result<Vec<u8>, RuntimeError> {
        self.fs_view(None)?.read_file_range(path, offset, max_bytes).await
    }

    /// The canonical path of an existing host path.
    pub async fn fs_canonical_path(self: Arc<Self>, path: String) -> Result<String, RuntimeError> {
        self.fs_view(None)?.canonical_path(path).await
    }

    /// Read a host file as UTF-8 text.
    pub async fn fs_read_text_file(self: Arc<Self>, path: String) -> Result<String, RuntimeError> {
        self.fs_view(None)?.read_text_file(path).await
    }

    /// Create or replace a host file.
    pub async fn fs_write_file(self: Arc<Self>, path: String, data: Vec<u8>) -> Result<(), RuntimeError> {
        self.fs_view(None)?.write_file(path, data).await
    }

    /// Append to a host file.
    pub async fn fs_append_file(self: Arc<Self>, path: String, data: Vec<u8>) -> Result<(), RuntimeError> {
        self.fs_view(None)?.append_file(path, data).await
    }

    /// Metadata for a host path, following a final symlink.
    pub async fn fs_stat(self: Arc<Self>, path: String) -> Result<FsMetadata, RuntimeError> {
        self.fs_view(None)?.stat(path).await
    }

    /// Metadata for a host path without following a final symlink.
    pub async fn fs_lstat(self: Arc<Self>, path: String) -> Result<FsMetadata, RuntimeError> {
        self.fs_view(None)?.lstat(path).await
    }

    /// Names of a host directory's direct children.
    pub async fn fs_read_dir(self: Arc<Self>, path: String) -> Result<Vec<String>, RuntimeError> {
        self.fs_view(None)?.read_dir(path).await
    }

    /// The target of a host symlink.
    pub async fn fs_read_link(self: Arc<Self>, path: String) -> Result<String, RuntimeError> {
        self.fs_view(None)?.read_link(path).await
    }

    /// Create a host directory.
    pub async fn fs_make_dir(self: Arc<Self>, path: String, recursive: bool) -> Result<(), RuntimeError> {
        self.fs_view(None)?.make_dir(path, recursive).await
    }

    /// Remove a host file or directory.
    pub async fn fs_remove(self: Arc<Self>, path: String, recursive: bool) -> Result<(), RuntimeError> {
        self.fs_view(None)?.remove(path, recursive).await
    }

    /// Rename a host path.
    pub async fn fs_rename(self: Arc<Self>, from: String, to: String) -> Result<(), RuntimeError> {
        self.fs_view(None)?.rename(from, to).await
    }

    /// Whether a host path exists.
    pub async fn fs_exists(self: Arc<Self>, path: String) -> Result<bool, RuntimeError> {
        self.fs_view(None)?.exists(path).await
    }

    /// Construct a local, capability-restricted engine for synchronous foreign
    /// callers. Equivalent to `create` with only `limits` set.
    #[uniffi::constructor]
    pub fn create_stateless(
        heap_memory_max_mb: u64,
        execution_timeout_secs: u64,
    ) -> Result<Arc<Self>, RuntimeError> {
        Self::create(EngineConfig {
            limits: ExecutionLimits {
                heap_memory_max_mb,
                execution_timeout_secs,
                max_concurrent_executions: None,
            },
            data_dir: None,
            filesystem: None,
            heap_store: None,
            fs_snapshot_store: None,
            fs_labels_db: None,
            wasm_modules: None,
        })
    }

    /// Construct an engine from a builder-assembled [`EngineConfig`]. Each axis
    /// is independent: limits, hook-gated filesystem access, heap persistence,
    /// and filesystem snapshots. Storage lives under `data_dir`, or under a
    /// temporary directory removed with the engine. The engine owns a Tokio
    /// runtime, so synchronous foreign callers need no executor of their own.
    #[uniffi::constructor]
    pub fn create(config: EngineConfig) -> Result<Arc<Self>, RuntimeError> {
        let limits = config.limits;
        if !(16..=4096).contains(&limits.heap_memory_max_mb)
            || !(1..=300).contains(&limits.execution_timeout_secs)
        {
            return Err(RuntimeError::InvalidConfig {
                message: "heap_memory_max_mb must be 16..=4096 and execution_timeout_secs must be 1..=300".into(),
            });
        }
        let max_concurrent = limits.max_concurrent_executions.unwrap_or(1);
        if !(1..=64).contains(&max_concurrent) {
            return Err(RuntimeError::InvalidConfig {
                message: "max_concurrent_executions must be 1..=64".into(),
            });
        }
        let heap_memory_max_bytes = usize::try_from(limits.heap_memory_max_mb * 1024 * 1024)
            .map_err(|_| RuntimeError::InvalidConfig {
                message: "heap limit exceeds platform capacity".into(),
            })?;

        // WASM modules are engine-level: heap snapshots bake the compiled
        // modules in, so an engine has either a heap store or modules.
        let wasm_modules = config
            .wasm_modules
            .unwrap_or_default()
            .into_iter()
            .map(|module| {
                let bytes = std::fs::read(&module.path).map_err(|e| RuntimeError::InvalidConfig {
                    message: format!("failed to read WASM module '{}' from {}: {e}", module.name, module.path),
                })?;
                let max_memory_bytes = module
                    .max_memory_bytes
                    .map(usize::try_from)
                    .transpose()
                    .map_err(|_| RuntimeError::InvalidConfig {
                        message: format!("WASM module '{}': max_memory_bytes is too large", module.name),
                    })?;
                Ok(WasmModule {
                    name: module.name,
                    bytes,
                    max_memory_bytes,
                    description: module.description,
                })
            })
            .collect::<Result<Vec<_>, RuntimeError>>()?;
        if config.heap_store.is_some() && !wasm_modules.is_empty() {
            return Err(RuntimeError::InvalidConfig {
                message: "wasm_modules cannot be combined with heap_store: heap snapshots bake compiled modules in"
                    .into(),
            });
        }

        // Filesystem authority is explicit: an empty chain would permit everything.
        let filesystem = config
            .filesystem
            .as_ref()
            .map(|access| {
                let policies: opa::OperationPolicies = serde_json::from_str(&access.policies_json)
                    .map_err(|error| RuntimeError::InvalidConfig {
                        message: format!("filesystem policies_json: {error}"),
                    })?;
                if policies.policies.is_empty() && policies.pre.is_empty() && policies.stack.is_empty() {
                    return Err(RuntimeError::InvalidConfig {
                        message: "filesystem configuration must declare policies, pre hooks, or a stack".into(),
                    });
                }
                Ok(policies)
            })
            .transpose()?;

        let (data_dir, ephemeral) = match config.data_dir {
            Some(path) => (path, None),
            None => {
                let directory = tempfile::tempdir().map_err(|e| RuntimeError::Initialization {
                    message: e.to_string(),
                })?;
                let path = directory.path().to_str().ok_or_else(|| RuntimeError::Initialization {
                    message: "temporary path is not UTF-8".into(),
                })?;
                (path.to_string(), Some(directory))
            }
        };

        let heap = blob_store_parts(config.heap_store.as_ref(), &format!("{data_dir}/heaps"))?;
        let fs = blob_store_parts(config.fs_snapshot_store.as_ref(), &format!("{data_dir}/fs-blobs"))?;
        let (s3_bucket, cache_dir) = match (&heap.s3, &fs.s3) {
            (Some(a), Some(b)) if a != b => {
                return Err(RuntimeError::InvalidConfig {
                    message: "heap_store and fs_snapshot_store must use the same S3 bucket and cache directory".into(),
                });
            }
            (Some(a), _) | (None, Some(a)) => a.clone(),
            (None, None) => (None, None),
        };
        let storage = StorageBootstrapConfig {
            heap_store: heap.kind,
            heap_dir: heap.dir,
            fs_store: fs.kind,
            fs_dir: fs.dir,
            fs_labels_db: config.fs_labels_db,
            s3_bucket,
            cache_dir,
            session_db_path: data_dir.clone(),
            http_port: None,
            execution_db_path: Some(format!("{data_dir}/executions")),
            heap_memory_max_bytes,
            execution_timeout_secs: limits.execution_timeout_secs,
            max_concurrent_executions: usize::try_from(max_concurrent).unwrap_or(1),
            session_id: None,
            session_fork_from: None,
        };

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|e| RuntimeError::Initialization {
                message: e.to_string(),
            })?;
        let filesystem_passthrough = config
            .filesystem
            .as_ref()
            .and_then(|access| access.passthrough)
            .unwrap_or(false);
        // The storage bootstrap is async (S3 clients capture the runtime they
        // are built on). Drive it on the engine's own runtime from a helper
        // thread so `create` works whether or not the caller is inside Tokio.
        let bootstrap = (|| -> Result<RuntimeBootstrap, RuntimeError> {
            let bootstrap = std::thread::scope(|scope| {
                scope
                    .spawn(|| runtime.block_on(build_storage_engine(storage, None)))
                    .join()
                    .map_err(|_| RuntimeError::Initialization {
                        message: "storage bootstrap panicked".into(),
                    })
            })?
            .map_err(|error| RuntimeError::Initialization {
                message: error.to_string(),
            })?;
            bootstrap
                .with_feature_config(FeatureBootstrapConfig {
                    wasm_modules,
                    ..FeatureBootstrapConfig::default()
                })?
                .with_policy_config(
                    PolicyBootstrapConfig {
                        filesystem,
                        ..PolicyBootstrapConfig::default()
                    },
                    CapabilityBootstrapConfig {
                        filesystem_passthrough,
                        ..CapabilityBootstrapConfig::default()
                    },
                )
        })();
        let bootstrap = match bootstrap {
            Ok(bootstrap) => bootstrap,
            Err(error) => {
                // A bare runtime must not be dropped inside an async caller;
                // release it the way OwnedRuntime does.
                runtime.shutdown_background();
                return Err(error);
            }
        };
        let mut engine = bootstrap.build_with_runtime(runtime);
        // The server bootstrap degrades gracefully when a store cannot be
        // opened (a warning, then reduced capabilities). An embedded engine
        // must instead fail: a caller that asked for persistence under a data
        // directory would otherwise silently lose it, typically because another
        // engine still holds the same directory.
        let stateful = engine.heap_enabled() || engine.fs_enabled();
        let unavailable = if engine.execution_registry.is_none() {
            Some("execution registry")
        } else if stateful && engine.session_log.is_none() {
            Some("session log")
        } else if engine.heap_enabled() && engine.heap_tag_store.is_none() {
            Some("heap tag store")
        } else {
            None
        };
        if let Some(store) = unavailable {
            return Err(RuntimeError::Initialization {
                message: format!(
                    "could not open the {store} under {data_dir}; is another engine using this data directory?"
                ),
            });
        }
        Arc::get_mut(&mut engine)
            .ok_or_else(|| RuntimeError::Initialization {
                message: "new embedded engine unexpectedly shared".into(),
            })?
            ._ephemeral_data_dir = ephemeral.map(Arc::new);
        Ok(engine)
    }

    /// Synchronous counterpart of shutdown for Python callers.
    pub fn close(&self) -> Result<RuntimeShutdownResult, RuntimeError> {
        if tokio::runtime::Handle::try_current().is_ok() {
            return Err(RuntimeError::Operation {
                message: "close must be called outside a Tokio runtime; use shutdown instead"
                    .into(),
            });
        }
        let runtime = self
            .tokio_runtime
            .as_ref()
            .ok_or_else(|| RuntimeError::Initialization {
                message: "synchronous close requires a library-created runtime".into(),
            })?;
        Ok(runtime.block_on(self.shutdown()))
    }

    pub fn mode(&self) -> RuntimeMode {
        if self.session_capable() {
            RuntimeMode::LocalStateful
        } else {
            RuntimeMode::Stateless
        }
    }

    pub fn lifecycle_state(&self) -> RuntimeLifecycleState {
        self.current_lifecycle_state()
    }

    pub async fn shutdown(&self) -> RuntimeShutdownResult {
        let _guard = self.shutdown_lock.lock().await;
        if self.current_lifecycle_state() == RuntimeLifecycleState::Shutdown {
            return RuntimeShutdownResult {
                cancelled_executions: 0,
                closed_mcp_connections: 0,
                cluster_shutdown: false,
                already_shutdown: true,
            };
        }

        self.lifecycle
            .store(RuntimeLifecycleState::ShuttingDown as u8, Ordering::Release);
        let (cancelled_executions, closed_mcp_connections) = self.shutdown_background_tasks().await;
        let cluster_shutdown = self.cluster_node.as_ref().is_some_and(|node| {
            node.shutdown();
            true
        });
        self.lifecycle
            .store(RuntimeLifecycleState::Shutdown as u8, Ordering::Release);

        RuntimeShutdownResult {
            cancelled_executions,
            closed_mcp_connections,
            cluster_shutdown,
            already_shutdown: false,
        }
    }

    pub fn capabilities(&self) -> RuntimeCapabilities {
        RuntimeCapabilities {
            heap: self.heap_enabled(),
            filesystem: self.fs_enabled(),
            sessions: self.session_capable(),
        }
    }

    pub async fn submit_execution(
        &self,
        request: ExecutionRequest,
    ) -> Result<String, RuntimeError> {
        let _lifecycle_guard = self.shutdown_lock.lock().await;
        self.ensure_running()?;
        if request.code.is_empty() && request.file.is_none() {
            return Err(RuntimeError::InvalidConfig {
                message: "execution requires code or a file path".to_string(),
            });
        }
        if !request.code.is_empty() && request.file.is_some() {
            return Err(RuntimeError::InvalidConfig {
                message: "execution cannot specify both code and a file path".to_string(),
            });
        }
        let heap_memory_max_mb = request
            .heap_memory_max_mb
            .map(usize::try_from)
            .transpose()
            .map_err(|_| RuntimeError::InvalidConfig {
                message: "heap_memory_max_mb is too large for this platform".to_string(),
            })?;
        let mcp_headers = request.mcp_headers.map(mcp_headers_value);

        let engine = self.clone();
        self.on_runtime(async move {
            let mut execution = engine
                .run_js(request.code)
                .maybe_file(request.file)
                .maybe_fs(request.fs)
                .maybe_session(request.session)
                .maybe_mcp_headers(mcp_headers);
            if let Some(heap) = request.heap {
                execution = execution.heap(heap);
            }
            if let Some(heap_memory_max_mb) = heap_memory_max_mb {
                execution = execution.heap_memory_max_mb(heap_memory_max_mb);
            }
            if let Some(execution_timeout_secs) = request.execution_timeout_secs {
                execution = execution.execution_timeout_secs(execution_timeout_secs);
            }
            if let Some(tags) = request.tags {
                execution = execution.tags(tags);
            }
            execution.execute().await.map_err(operation_message)
        })
        .await
    }

    pub fn get_execution(&self, execution_id: String) -> Result<ExecutionInfo, RuntimeError> {
        self.execution_registry()?
            .get(&execution_id)
            .ok_or_else(|| operation_message(format!("Execution '{}' not found", execution_id)))
    }

    pub fn get_execution_output(
        &self,
        execution_id: String,
        line_offset: Option<u64>,
        line_limit: Option<u64>,
        byte_offset: Option<u64>,
        byte_limit: Option<u64>,
    ) -> Result<ConsoleOutputPage, RuntimeError> {
        self.execution_registry()?
            .get_console_output(
                &execution_id,
                line_offset,
                line_limit,
                byte_offset,
                byte_limit,
            )
            .map_err(operation_message)
    }

    pub fn cancel_execution(&self, execution_id: String) -> Result<(), RuntimeError> {
        self.execution_registry()?
            .cancel(&execution_id)
            .map_err(operation_message)
    }

    pub fn list_executions(&self) -> Result<Vec<ExecutionSummary>, RuntimeError> {
        Ok(self.execution_registry()?.list())
    }

    pub async fn list_sessions(&self) -> Result<Vec<String>, RuntimeError> {
        self.session_log()?
            .list_sessions()
            .await
            .map_err(operation_message)
    }

    pub async fn list_session_snapshots(
        &self,
        session: String,
    ) -> Result<Vec<session_log::SessionSnapshotView>, RuntimeError> {
        self.session_log()?
            .list_entries(&session)
            .await
            .map_err(operation_message)
    }

    pub async fn get_heap_tags(
        &self,
        heap: String,
    ) -> Result<HashMap<String, String>, RuntimeError> {
        self.heap_tag_store()?
            .get_tags(&heap)
            .await
            .map_err(operation_message)
    }

    pub async fn set_heap_tags(
        &self,
        heap: String,
        tags: HashMap<String, String>,
    ) -> Result<(), RuntimeError> {
        self.heap_tag_store()?
            .set_tags(&heap, tags)
            .await
            .map_err(operation_message)
    }

    pub async fn delete_heap_tags(
        &self,
        heap: String,
        keys: Option<Vec<String>>,
    ) -> Result<(), RuntimeError> {
        self.heap_tag_store()?
            .delete_tags(&heap, keys)
            .await
            .map_err(operation_message)
    }

    pub async fn query_heaps_by_tags(
        &self,
        tags: HashMap<String, String>,
    ) -> Result<Vec<HeapTagEntry>, RuntimeError> {
        self.heap_tag_store()?
            .query_by_tags(tags)
            .await
            .map_err(operation_message)
    }

    /// List every label and its current head CA id (hex).
    pub async fn fs_list_labels(&self) -> Result<Vec<FsLabelView>, RuntimeError> {
        let result: Result<Vec<FsLabelView>, String> = async {
            let labels = self.labels_or_err()?;
            Ok(labels
                .list()
                .await?
                .into_iter()
                .map(|(name, id)| FsLabelView {
                    name,
                    ca_id: ca_to_hex(&id),
                })
                .collect())
        }
        .await;
        result.map_err(operation_message)
    }

    /// Resolve a label to its current head CA id (hex), if it exists.
    pub async fn fs_resolve_label(&self, name: String) -> Result<Option<String>, RuntimeError> {
        let result: Result<Option<String>, String> = async {
            let labels = self.labels_or_err()?;
            Ok(labels.resolve(&name).await?.map(|id| ca_to_hex(&id)))
        }
        .await;
        result.map_err(operation_message)
    }

    /// Create a label, or repoint an existing one, to a CA id. `message` is an
    /// optional human note recorded on the reflog entry.
    pub async fn fs_set_label(
        &self,
        name: String,
        ca_id: String,
        message: Option<String>,
    ) -> Result<(), RuntimeError> {
        let result: Result<(), String> = async {
            self.check_fs_snapshot_policy("label", Some(&name), Some(&ca_id))
                .await?;
            let labels = self.labels_or_err()?;
            let id = parse_ca_hex(&ca_id).ok_or_else(|| format!("invalid CA id: {ca_id}"))?;
            match labels.resolve(&name).await? {
                Some(_) => labels.force(&name, id, message).await,
                None => labels.create(&name, id, message).await,
            }
        }
        .await;
        result.map_err(operation_message)
    }

    /// The reflog for a label (hex-rendered), oldest first. When `limit` is
    /// given, only the most recent `limit` entries are read and returned.
    pub async fn fs_label_log(
        &self,
        name: String,
        limit: Option<u64>,
    ) -> Result<Vec<FsRefLogView>, RuntimeError> {
        let limit =
            limit
                .map(usize::try_from)
                .transpose()
                .map_err(|_| RuntimeError::Operation {
                    message: "filesystem log limit is too large for this platform".to_string(),
                })?;
        let result: Result<Vec<FsRefLogView>, String> = async {
            let labels = self.labels_or_err()?;
            let entries = match limit {
                Some(n) => labels.log_recent(&name, n).await?,
                None => labels.log(&name).await?,
            };
            Ok(entries
                .into_iter()
                .map(|e| FsRefLogView {
                    at: e.at,
                    from: e.from.as_ref().map(ca_to_hex),
                    to: ca_to_hex(&e.to),
                    op: refop_str(e.op).to_string(),
                    message: e.message,
                })
                .collect())
        }
        .await;
        result.map_err(operation_message)
    }

    /// Advance a label to a CA id. Default is reject-and-rebase: the move only
    /// succeeds if the label's current head equals `expected` (or the label does
    /// not yet exist and `expected` is `None`). `force` skips the check.
    pub async fn fs_push(
        &self,
        label: String,
        ca_id: String,
        expected: Option<String>,
        force: bool,
        message: Option<String>,
    ) -> Result<FsPushOutcome, RuntimeError> {
        let result: Result<FsPushOutcome, String> = async {
            self.check_fs_snapshot_policy("push", Some(&label), Some(&ca_id))
                .await?;
            let labels = self.labels_or_err()?;
            let new = parse_ca_hex(&ca_id).ok_or_else(|| format!("invalid CA id: {ca_id}"))?;

            if force {
                labels.force(&label, new, message).await?;
                return Ok(FsPushOutcome::Advanced {
                    label: label.clone(),
                    ca_id: ca_id.clone(),
                });
            }

            let expected = match expected {
                Some(h) => {
                    Some(parse_ca_hex(&h).ok_or_else(|| format!("invalid expected CA id: {h}"))?)
                }
                None => None,
            };
            let current = labels.resolve(&label).await?;
            let advanced = if current.is_none() && expected.is_none() {
                labels.create(&label, new, message).await?;
                true
            } else {
                labels.cas(&label, expected, new, message).await?
            };

            if advanced {
                Ok(FsPushOutcome::Advanced {
                    label: label.clone(),
                    ca_id: ca_id.clone(),
                })
            } else {
                Ok(FsPushOutcome::Rejected {
                    label: label.clone(),
                    current: current.as_ref().map(ca_to_hex),
                })
            }
        }
        .await;
        result.map_err(operation_message)
    }

    /// Reset a label to an earlier CA id from its reflog (the rollback verb).
    /// Unless `allow_unlogged` is set, the target must appear in the label's
    /// reflog so resets stay within recorded history.
    pub async fn fs_reset(
        &self,
        label: String,
        ca_id: String,
        allow_unlogged: bool,
        message: Option<String>,
    ) -> Result<(), RuntimeError> {
        let result: Result<(), String> = async {
            self.check_fs_snapshot_policy("reset", Some(&label), Some(&ca_id))
                .await?;
            let labels = self.labels_or_err()?;
            let target = parse_ca_hex(&ca_id).ok_or_else(|| format!("invalid CA id: {ca_id}"))?;
            if !allow_unlogged {
                let in_log = labels
                    .log(&label)
                    .await?
                    .iter()
                    .any(|e| e.to == target || e.from == Some(target));
                if !in_log {
                    return Err(format!(
                        "CA id {ca_id} is not in the reflog for label '{label}'; \
                         pass allow_unlogged to reset anyway"
                    ));
                }
            }
            labels.force(&label, target, message).await
        }
        .await;
        result.map_err(operation_message)
    }

    /// Three-way merge two snapshots into a new one. Structural merge prunes
    /// equal subtrees by hash; a content-merge pass resolves text conflicts
    /// before reporting the rest with diffs/markers.
    pub async fn fs_merge(
        &self,
        ours: String,
        theirs: String,
        base: Option<String>,
        prefer: Prefer,
    ) -> Result<FsMergeResult, RuntimeError> {
        let result: Result<FsMergeResult, String> = async {
            self.check_fs_snapshot_policy("merge", None, None).await?;
            let store = self.fs_store_or_err()?;

            let load = |hex: &str| -> Result<[u8; 32], String> {
                parse_ca_hex(hex).ok_or_else(|| format!("invalid CA id: {hex}"))
            };
            let base_root = match &base {
                Some(b) => Some(load(b)?),
                None => None,
            };

            let structural = fs_merge::merge_trees(
                store,
                base_root,
                Some(load(&ours)?),
                Some(load(&theirs)?),
                prefer,
            )
            .await
            .map_err(|e| format!("fs_merge: {e}"))?;
            let merged_root = structural.root;

            let mergers = fs_content_merge::default_mergers();
            let mut conflict_views = Vec::new();
            let mut resolved: Vec<(Vec<String>, Option<fs_store::Entry>)> = Vec::new();
            for c in structural.conflicts {
                let view = match (&c.ours, &c.theirs) {
                    (Some(oe), Some(te)) => {
                        let ours_b = store.read_file(oe).await.map_err(|e| {
                            format!("fs_merge: read ours {}: {e}", c.path.display())
                        })?;
                        let theirs_b = store.read_file(te).await.map_err(|e| {
                            format!("fs_merge: read theirs {}: {e}", c.path.display())
                        })?;
                        let base_b = match &c.base {
                            Some(be) => Some(store.read_file(be).await.map_err(|e| {
                                format!("fs_merge: read base {}: {e}", c.path.display())
                            })?),
                            None => None,
                        };
                        match fs_content_merge::merge_content(
                            &mergers,
                            base_b.as_deref(),
                            &ours_b,
                            &theirs_b,
                        ) {
                            fs_content_merge::ContentMergeResult::Clean(bytes) => {
                                let entry = store.put_file(&bytes).await.map_err(|e| {
                                    format!("fs_merge: store merged {}: {e}", c.path.display())
                                })?;
                                resolved.push((fs_tree::components_of(&c.path), Some(entry)));
                                continue; // resolved — not a conflict
                            }
                            fs_content_merge::ContentMergeResult::Conflict(cc) => {
                                FsMergeConflictView {
                                    path: c.path.to_string_lossy().to_string(),
                                    base: c.base.as_ref().map(entry_content_id),
                                    ours: c.ours.as_ref().map(entry_content_id),
                                    theirs: c.theirs.as_ref().map(entry_content_id),
                                    kind: cc.kind.as_str().to_string(),
                                    markers: cc.markers,
                                    diff_ours: cc.diff_ours,
                                    diff_theirs: cc.diff_theirs,
                                }
                            }
                        }
                    }
                    // A modify/delete (or add on one side): no content to reconcile.
                    _ => FsMergeConflictView {
                        path: c.path.to_string_lossy().to_string(),
                        base: c.base.as_ref().map(entry_content_id),
                        ours: c.ours.as_ref().map(entry_content_id),
                        theirs: c.theirs.as_ref().map(entry_content_id),
                        kind: "modify/delete".to_string(),
                        markers: None,
                        diff_ours: None,
                        diff_theirs: None,
                    },
                };
                conflict_views.push(view);
            }

            if conflict_views.is_empty() {
                let final_root = if resolved.is_empty() {
                    merged_root
                } else {
                    store
                        .build_root(Some(merged_root), resolved)
                        .await
                        .map_err(|e| format!("fs_merge: store result: {e}"))?
                };
                Ok(FsMergeResult::Merged {
                    ca_id: ca_to_hex(&final_root),
                })
            } else {
                Ok(FsMergeResult::Conflict {
                    conflicts: conflict_views,
                })
            }
        }
        .await;
        result.map_err(operation_message)
    }

    pub fn list_tools(&self) -> Result<Vec<ToolDefinition>, RuntimeError> {
        self.mcp_tools()
            .into_iter()
            .map(|tool| {
                let input_schema_json =
                    serde_json::to_string(tool.input_schema.as_ref()).map_err(|error| {
                        RuntimeError::Initialization {
                            message: format!(
                                "failed to serialize schema for '{}': {error}",
                                tool.name
                            ),
                        }
                    })?;
                Ok(ToolDefinition {
                    name: tool.name.to_string(),
                    description: tool.description.map(|description| description.to_string()),
                    input_schema_json,
                })
            })
            .collect()
    }

    pub fn call_tool(
        &self,
        name: String,
        arguments_json: String,
        session_id: Option<String>,
        mcp_headers: Option<McpRequestHeaders>,
    ) -> Result<String, RuntimeError> {
        if tokio::runtime::Handle::try_current().is_ok() {
            return Err(RuntimeError::Operation {
                message:
                    "call_tool must be called outside a Tokio runtime; use invoke_tool instead"
                        .into(),
            });
        }
        let tokio_runtime =
            self.tokio_runtime
                .as_ref()
                .ok_or_else(|| RuntimeError::Initialization {
                    message: "synchronous tool calls require a library-created runtime".to_string(),
                })?;
        tokio_runtime.block_on(self.invoke_tool(ToolCallRequest {
            name,
            arguments_json,
            session_id,
            mcp_headers,
        }))
    }

    /// Invoke a tool without blocking the foreign caller's host thread.
    pub async fn call_tool_async(
        self: Arc<Self>,
        name: String,
        arguments_json: String,
        session_id: Option<String>,
        mcp_headers: Option<McpRequestHeaders>,
    ) -> Result<String, RuntimeError> {
        let request = ToolCallRequest {
            name,
            arguments_json,
            session_id,
            mcp_headers,
        };
        if tokio::runtime::Handle::try_current().is_ok() {
            return self.invoke_tool(request).await;
        }
        let tokio_runtime = self
            .tokio_runtime
            .as_ref()
            .ok_or_else(|| RuntimeError::Initialization {
                message: "asynchronous tool calls require an active or library-created runtime"
                    .to_string(),
            })?;
        let engine = self.clone();
        tokio_runtime
            .spawn(async move { engine.invoke_tool(request).await })
            .await
            .map_err(|error| RuntimeError::Operation {
                message: format!("asynchronous tool call task failed: {error}"),
            })?
    }

    pub async fn invoke_tool(&self, request: ToolCallRequest) -> Result<String, RuntimeError> {
        let result = self.invoke_tool_response(request).await?;
        serde_json::to_string(&result.json).map_err(|error| RuntimeError::ToolCall {
            message: format!("failed to serialize result: {error}"),
        })
    }
}

impl Engine {
    /// Preserve artifact content blocks for Rust transports without serializing them through JSON.
    pub async fn invoke_tool_response(
        &self,
        request: ToolCallRequest,
    ) -> Result<crate::mcp_dispatch::ToolResponse, RuntimeError> {
        let _lifecycle_guard = self.shutdown_lock.lock().await;
        self.ensure_running()?;
        let arguments = parse_json_object("arguments_json", &request.arguments_json)?;
        let mcp_headers = request.mcp_headers.map(mcp_headers_value);
        let result = self
            .dispatch_tool(
                request.session_id.as_deref(),
                mcp_headers.as_ref(),
                &request.name,
                &arguments,
            )
            .await;

        Ok(result)
    }

    /// Run a future on the caller's runtime when there is one and otherwise on
    /// the library-created runtime, without blocking the foreign host thread.
    async fn on_runtime<T, Fut>(&self, operation: Fut) -> Result<T, RuntimeError>
    where
        T: Send + 'static,
        Fut: std::future::Future<Output = Result<T, RuntimeError>> + Send + 'static,
    {
        if tokio::runtime::Handle::try_current().is_ok() {
            return operation.await;
        }
        let tokio_runtime = self
            .tokio_runtime
            .as_ref()
            .ok_or_else(|| RuntimeError::Initialization {
                message: "native calls require an active or library-created runtime".to_string(),
            })?;
        tokio_runtime
            .spawn(operation)
            .await
            .map_err(|error| RuntimeError::Operation {
                message: format!("native task failed: {error}"),
            })?
    }

    /// Run one native filesystem operation under the lifecycle guard, so
    /// shutdown waits for it as it does for tool calls.
    async fn run_native<T, Fut>(&self, operation: Fut) -> Result<T, RuntimeError>
    where
        T: Send + 'static,
        Fut: std::future::Future<Output = Result<T, RuntimeError>> + Send + 'static,
    {
        let _lifecycle_guard = self.shutdown_lock.lock().await;
        self.ensure_running()?;
        self.on_runtime(operation).await
    }

    /// The host filesystem service native callers share with guest `fs.*`
    /// calls: the same hook chain and headers.
    fn host_fs_service(&self) -> Result<fs::FsService, RuntimeError> {
        let config = self.fs_config.as_ref().ok_or_else(|| RuntimeError::Operation {
            message: "filesystem access is not configured; set EngineConfig.filesystem".into(),
        })?;
        Ok(fs::FsService::host((**config).clone()))
    }

    /// Run one native operation on a session's filesystem snapshot: mount the
    /// session's latest snapshot (or an empty overlay), run the operation
    /// through the shared service, and for a mutation fold the overlay into a
    /// new snapshot recorded in the session log with the session's current
    /// heap, so the next `run_js` in the session resumes both.
    async fn session_fs_run<T, F, Fut>(
        &self,
        session: String,
        mutation: Option<String>,
        op: F,
    ) -> Result<T, RuntimeError>
    where
        F: FnOnce(fs::FsService) -> Fut,
        Fut: std::future::Future<Output = Result<T, fs::FsError>>,
    {
        let store = self.fs_store.clone().ok_or_else(|| RuntimeError::Operation {
            message: "filesystem snapshots are not configured".into(),
        })?;
        let log = self.session_log.as_ref().ok_or_else(|| RuntimeError::Operation {
            message: "session log is not configured".into(),
        })?;
        let config = self.fs_config.clone().ok_or_else(|| RuntimeError::Operation {
            message: "filesystem access is not configured".into(),
        })?;
        let _serial = self.native_fs_session_lock.lock().await;
        let latest = log.get_latest(&session).await.map_err(operation_message)?;
        let base = latest.as_ref().and_then(|entry| entry.output_fs.clone());
        self.check_fs_snapshot_policy("pull", Some(&session), base.as_deref())
            .await
            .map_err(operation_message)?;
        let mount = match base.as_deref().and_then(parse_ca_hex) {
            Some(id) => SessionMount::pull((*store).clone(), blake3::Hash::from_bytes(id))
                .await
                .map_err(|e| operation_message(format!("fs mount: pull {}: {e}", base.as_deref().unwrap_or(""))))?,
            None => SessionMount::empty((*store).clone()),
        };
        let handle = fs::FsMountHandle::new(mount);
        let value = op(fs::FsService::new((*config).clone(), Some(handle.clone()))).await?;
        if let Some(description) = mutation {
            let root = handle
                .0
                .lock()
                .await
                .push()
                .await
                .map_err(|e| operation_message(format!("fs snapshot flush failed: {e}")))?;
            let output_fs = ca_to_hex(root.as_bytes());
            // A no-op mutation (a mkdir on the overlay, say) folds to the same
            // root and needs no log entry.
            if base.as_deref() != Some(output_fs.as_str()) {
                self.check_fs_snapshot_policy("push", Some(&session), Some(&output_fs))
                    .await
                    .map_err(operation_message)?;
                let output_heap = latest.as_ref().map(|entry| entry.output_heap.clone()).unwrap_or_default();
                log.append(
                    &session,
                    SessionLogEntry {
                        input_heap: Some(output_heap.clone()).filter(|heap| !heap.is_empty()),
                        output_heap,
                        output_fs: Some(output_fs),
                        code: description,
                        timestamp: chrono::Utc::now().to_rfc3339(),
                    },
                )
                .await
                .map_err(operation_message)?;
            }
        }
        Ok(value)
    }

    /// Wrap a fully configured runtime for Rust transports without creating a
    /// second Tokio executor or crossing the FFI boundary.
    fn wrap(
        mut engine: Engine,
        tokio_runtime: Option<tokio::runtime::Runtime>,
        ephemeral_data_dir: Option<tempfile::TempDir>,
        cluster_node: Option<Arc<ClusterNode>>,
    ) -> Arc<Self> {
        engine.tokio_runtime = tokio_runtime.map(|runtime| Arc::new(OwnedRuntime(Some(runtime))));
        engine.cluster_node = cluster_node;
        engine._ephemeral_data_dir = ephemeral_data_dir.map(Arc::new);
        Arc::new(engine)
    }

    fn current_lifecycle_state(&self) -> RuntimeLifecycleState {
        match self.lifecycle.load(Ordering::Acquire) {
            value if value == RuntimeLifecycleState::Running as u8 => {
                RuntimeLifecycleState::Running
            }
            value if value == RuntimeLifecycleState::ShuttingDown as u8 => {
                RuntimeLifecycleState::ShuttingDown
            }
            _ => RuntimeLifecycleState::Shutdown,
        }
    }

    fn execution_registry(&self) -> Result<&ExecutionRegistry, RuntimeError> {
        self.execution_registry
            .as_deref()
            .ok_or_else(|| operation_message("Execution registry not configured".to_string()))
    }

    fn session_log(&self) -> Result<&SessionLog, RuntimeError> {
        self.session_log
            .as_ref()
            .ok_or_else(|| operation_message("Session log not configured".to_string()))
    }

    fn heap_tag_store(&self) -> Result<&HeapTagStore, RuntimeError> {
        self.heap_tag_store
            .as_ref()
            .ok_or_else(|| operation_message("Heap tag store not configured".to_string()))
    }

    fn ensure_running(&self) -> Result<(), RuntimeError> {
        match self.current_lifecycle_state() {
            RuntimeLifecycleState::Running => Ok(()),
            state => Err(RuntimeError::Operation {
                message: format!("runtime is {state:?}"),
            }),
        }
    }

    pub fn from_engine(engine: Engine) -> Arc<Self> {
        Self::from_engine_with_cluster(engine, None)
    }

    pub(crate) fn from_engine_with_cluster(
        engine: Engine,
        cluster_node: Option<Arc<ClusterNode>>,
    ) -> Arc<Self> {
        Self::wrap(engine, None, None, cluster_node)
    }

    pub(crate) fn from_engine_with_tokio_runtime(
        engine: Engine,
        tokio_runtime: tokio::runtime::Runtime,
        cluster_node: Option<Arc<ClusterNode>>,
    ) -> Arc<Self> {
        Self::wrap(engine, Some(tokio_runtime), None, cluster_node)
    }

    pub fn tool_catalog(&self) -> ToolCatalog {
        built_in_tool_catalog(self.heap_enabled(), self.fs_enabled())
    }

    pub fn core_mcp_tools(&self) -> Vec<rmcp::model::Tool> {
        crate::mcp::mode_tool_list(self)
    }

    pub fn mcp_tools(&self) -> Vec<rmcp::model::Tool> {
        let mut tools = self.core_mcp_tools();
        tools.extend(self.upstream_mcp_stub_tools());
        tools.extend(self.wasm_stub_tools());
        tools
    }

    pub fn upstream_mcp_stub_tools(&self) -> Vec<rmcp::model::Tool> {
        self.mcp_client_manager()
            .map(|client| client.stub_tools())
            .unwrap_or_default()
    }

    /// Dispatch a tool call against the full MCP tool catalog.
    pub async fn dispatch_tool(
        &self,
        session_id: Option<&str>,
        mcp_headers: Option<&Value>,
        name: &str,
        arguments: &Value,
    ) -> crate::mcp_dispatch::ToolResponse {
        if self.session_capable() {
            crate::mcp_dispatch::call_tool(self, session_id, mcp_headers, name, arguments).await
        } else if name == "run_js" {
            crate::mcp_dispatch::run_js_blocking(self, mcp_headers, arguments).await
        } else if name == "get_artifact" {
            crate::mcp_dispatch::get_artifact(self, arguments)
        } else if name == "list_artifacts" {
            crate::mcp_dispatch::list_artifacts(self).into()
        } else {
            json!({ "error": format!("unknown stateless tool: {name}") }).into()
        }
    }

    pub fn upstream_mcp_stub_call_response(
        &self,
        name: &str,
        arguments: Option<&serde_json::Map<String, Value>>,
    ) -> Option<rmcp::model::CallToolResult> {
        self.mcp_client_manager()
            .and_then(|client| client.stub_call_response(name, arguments))
    }
}

fn operation_message(message: String) -> RuntimeError {
    RuntimeError::Operation { message }
}

fn mcp_headers_value(headers: McpRequestHeaders) -> Value {
    Value::Object(
        headers
            .values
            .into_iter()
            .map(|(name, value)| (name, Value::String(value)))
            .collect(),
    )
}

fn parse_json_object(field: &str, json: &str) -> Result<Value, RuntimeError> {
    let value: Value = serde_json::from_str(json).map_err(|error| RuntimeError::InvalidJson {
        field: field.to_string(),
        message: error.to_string(),
    })?;
    if !value.is_object() {
        return Err(RuntimeError::InvalidJson {
            field: field.to_string(),
            message: "expected a JSON object".to_string(),
        });
    }
    Ok(value)
}

/// A filesystem namespace for native callers: the host filesystem behind the
/// engine's hook chain, or one session's content-addressed snapshot. Obtained
/// from `Engine::fs_view`. Bytes cross the boundary as `bytes`; failures are
/// `RuntimeError::FileSystem` with the same message the guest wrapper reports.
#[derive(uniffi::Object)]
pub struct FsView {
    engine: Arc<Engine>,
    session: Option<String>,
}

impl FsView {
    async fn run<T, F, Fut>(self: Arc<Self>, mutation: Option<String>, op: F) -> Result<T, RuntimeError>
    where
        T: Send + 'static,
        F: FnOnce(fs::FsService) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<T, fs::FsError>> + Send + 'static,
    {
        let engine = self.engine.clone();
        match self.session.clone() {
            None => {
                let service = engine.host_fs_service()?;
                engine
                    .run_native(async move { op(service).await.map_err(RuntimeError::from) })
                    .await
            }
            Some(session) => {
                let inner = engine.clone();
                engine
                    .run_native(async move { inner.session_fs_run(session, mutation, op).await })
                    .await
            }
        }
    }
}

#[uniffi::export]
impl FsView {
    /// The session this view is bound to; `None` for the host filesystem.
    pub fn session(&self) -> Option<String> {
        self.session.clone()
    }

    pub async fn read_file(self: Arc<Self>, path: String) -> Result<Vec<u8>, RuntimeError> {
        self.run(None, |fs| async move { fs.read_file(&path).await }).await
    }

    /// Read at most `max_bytes` bytes starting at `offset`; fewer bytes are
    /// returned only at end of file.
    pub async fn read_file_range(
        self: Arc<Self>,
        path: String,
        offset: u64,
        max_bytes: u64,
    ) -> Result<Vec<u8>, RuntimeError> {
        self.run(None, move |fs| async move { fs.read_range(&path, offset, max_bytes).await }).await
    }

    /// Read a file as UTF-8 text; invalid UTF-8 is an `InvalidData` failure.
    pub async fn read_text_file(self: Arc<Self>, path: String) -> Result<String, RuntimeError> {
        self.run(None, |fs| async move { fs.read_text(&path).await }).await
    }

    pub async fn write_file(self: Arc<Self>, path: String, data: Vec<u8>) -> Result<(), RuntimeError> {
        let note = format!("// native fs.writeFile {path}");
        self.run(Some(note), |fs| async move { fs.write_file(&path, &data).await }).await
    }

    pub async fn append_file(self: Arc<Self>, path: String, data: Vec<u8>) -> Result<(), RuntimeError> {
        let note = format!("// native fs.appendFile {path}");
        self.run(Some(note), |fs| async move { fs.append_file(&path, &data).await }).await
    }

    /// Metadata following a final symlink (Node `fs.stat`).
    pub async fn stat(self: Arc<Self>, path: String) -> Result<FsMetadata, RuntimeError> {
        self.run(None, |fs| async move { fs.stat(&path, true).await.map(FsMetadata::from) }).await
    }

    /// Metadata without following a final symlink (Node `fs.lstat`).
    pub async fn lstat(self: Arc<Self>, path: String) -> Result<FsMetadata, RuntimeError> {
        self.run(None, |fs| async move { fs.stat(&path, false).await.map(FsMetadata::from) }).await
    }

    pub async fn read_dir(self: Arc<Self>, path: String) -> Result<Vec<String>, RuntimeError> {
        self.run(None, |fs| async move { fs.readdir(&path).await }).await
    }

    pub async fn read_link(self: Arc<Self>, path: String) -> Result<String, RuntimeError> {
        self.run(None, |fs| async move { fs.readlink(&path).await }).await
    }

    /// The canonical path of an existing path, gated as a `stat`. A snapshot
    /// has no symlink resolution and returns the path itself.
    pub async fn canonical_path(self: Arc<Self>, path: String) -> Result<String, RuntimeError> {
        self.run(None, |fs| async move { fs.canonical_path(&path).await }).await
    }

    pub async fn make_dir(self: Arc<Self>, path: String, recursive: bool) -> Result<(), RuntimeError> {
        let note = format!("// native fs.mkdir {path}");
        self.run(Some(note), move |fs| async move { fs.mkdir(&path, recursive).await }).await
    }

    pub async fn remove(self: Arc<Self>, path: String, recursive: bool) -> Result<(), RuntimeError> {
        let note = format!("// native fs.rm {path}");
        self.run(Some(note), move |fs| async move { fs.rm(&path, recursive).await }).await
    }

    pub async fn rename(self: Arc<Self>, from: String, to: String) -> Result<(), RuntimeError> {
        let note = format!("// native fs.rename {from} -> {to}");
        self.run(Some(note), |fs| async move { fs.rename(&from, &to).await }).await
    }

    /// Whether a path exists. Only the hook chain can fail this call.
    pub async fn exists(self: Arc<Self>, path: String) -> Result<bool, RuntimeError> {
        self.run(None, |fs| async move { fs.exists(&path).await }).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::ffi_config::{
        BlobStoreBuilder, EngineConfigBuilder, ExecutionLimitsBuilder, FilesystemAccessBuilder,
        WasmModuleFileBuilder,
    };

    #[test]
    fn typed_mcp_headers_convert_to_policy_json() {
        let headers = McpRequestHeaders {
            values: HashMap::from([
                ("session-id".to_string(), "session-123".to_string()),
                ("tenant".to_string(), "acme".to_string()),
            ]),
        };

        assert_eq!(
            mcp_headers_value(headers),
            serde_json::json!({
                "session-id": "session-123",
                "tenant": "acme",
            })
        );
    }

    #[test]
    fn rejects_non_object_arguments() {
        let error = parse_json_object("arguments_json", "[]").unwrap_err();
        assert!(error.to_string().contains("expected a JSON object"));
    }

    #[tokio::test]
    async fn stateless_engine_runs_js_through_the_exported_surface() {
        initialize_v8();
        let data_dir = tempfile::tempdir().unwrap();
        let registry =
            ExecutionRegistry::new(data_dir.path().join("reg").to_str().unwrap()).unwrap();
        let engine = Engine::from_engine(
            Engine::new_stateless(64 * 1024 * 1024, 30, 2)
                .with_execution_registry(Arc::new(registry)),
        );

        let result = engine
            .clone()
            .call_tool_async(
                "run_js".to_string(),
                r#"{"code":"console.log(6 * 7)"}"#.to_string(),
                None,
                None,
            )
            .await
            .unwrap();
        let value: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(value["output"], "42");
    }

    #[tokio::test]
    async fn native_filesystem_methods_share_the_guest_hook_chain() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_str().unwrap().to_string();
        let hook = dir.path().join("hooks.rego");
        let alias = format!("{root}/alias.bin");
        let redirected = format!("{root}/redirected.bin");
        std::fs::write(
            &hook,
            format!(
                "package mcp.filesystem\n\
                 pre := {{\"input\": object.union(input, {{\"path\": \"{redirected}\"}})}} if {{ input.path == \"{alias}\" }}\n\
                 pre := {{\"allow\": false, \"reason\": \"outside sandbox\"}} if {{ not startswith(input.path, \"{root}\") }}\n"
            ),
        )
        .unwrap();
        let config = serde_json::json!({"pre": [{"url": format!("file://{}", hook.display())}]}).to_string();
        let engine = Engine::create_with_filesystem(64, 5, config).unwrap();
        assert!(engine.host_filesystem_enabled());

        // A pre-hook path rewrite applies to native writes and reads alike, and
        // the guest reads the same bytes through the same chain.
        engine.clone().fs_write_file(alias.clone(), vec![0, 255, 10]).await.unwrap();
        assert!(!std::path::Path::new(&alias).exists());
        assert_eq!(std::fs::read(&redirected).unwrap(), vec![0, 255, 10]);
        assert_eq!(engine.clone().fs_read_file(alias.clone()).await.unwrap(), vec![0, 255, 10]);
        let code = format!(
            "console.log(JSON.stringify(Array.from(await fs.readFile({}))))",
            serde_json::to_string(&alias).unwrap()
        );
        let result = engine
            .clone()
            .call_tool_async(
                "run_js".to_string(),
                serde_json::json!({ "code": code }).to_string(),
                None,
                None,
            )
            .await
            .unwrap();
        let value: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(value["output"].as_str().unwrap().trim(), "[0,255,10]");

        let text = format!("{root}/notes.txt");
        engine.clone().fs_write_file(text.clone(), "héllo".as_bytes().to_vec()).await.unwrap();
        engine.clone().fs_append_file(text.clone(), " wörld".as_bytes().to_vec()).await.unwrap();
        assert_eq!(engine.clone().fs_read_text_file(text.clone()).await.unwrap(), "héllo wörld");
        assert_eq!(
            engine.clone().fs_read_file_range(text.clone(), 1, 4).await.unwrap(),
            "héllo wörld".as_bytes()[1..5].to_vec()
        );
        assert_eq!(engine.clone().fs_read_file_range(text.clone(), 100, 4).await.unwrap(), Vec::<u8>::new());
        assert_eq!(engine.clone().fs_canonical_path(text.clone()).await.unwrap(), std::fs::canonicalize(&text).unwrap().to_string_lossy());
        let stat = engine.clone().fs_stat(text.clone()).await.unwrap();
        assert_eq!(stat.kind, FsEntryKind::File);
        assert_eq!(stat.size, "héllo wörld".len() as u64);
        assert!(stat.modified_ms.is_some());
        engine.clone().fs_make_dir(format!("{root}/a/b"), true).await.unwrap();
        assert_eq!(engine.clone().fs_read_dir(format!("{root}/a")).await.unwrap(), vec!["b".to_string()]);
        engine.clone().fs_rename(text.clone(), format!("{root}/a/b/moved.txt")).await.unwrap();
        assert!(!engine.clone().fs_exists(text.clone()).await.unwrap());
        assert_eq!(engine.clone().fs_lstat(format!("{root}/a")).await.unwrap().kind, FsEntryKind::Directory);
        engine.clone().fs_remove(format!("{root}/a"), true).await.unwrap();
        assert!(!engine.clone().fs_exists(format!("{root}/a")).await.unwrap());

        // Typed failures.
        let missing = engine.clone().fs_read_file(format!("{root}/missing")).await.unwrap_err();
        assert!(
            matches!(missing, RuntimeError::FileSystem { kind: FsErrorKind::NotFound, .. }),
            "{missing}"
        );
        assert!(missing.message().contains("ENOENT"), "{missing}");
        let invalid = engine.clone().fs_read_text_file(redirected.clone()).await.unwrap_err();
        assert!(
            matches!(invalid, RuntimeError::FileSystem { kind: FsErrorKind::InvalidData, .. }),
            "{invalid}"
        );
        match engine.clone().fs_read_dir("/etc".to_string()).await.unwrap_err() {
            RuntimeError::FileSystem { kind, message } => {
                assert_eq!(kind, FsErrorKind::PermissionDenied);
                assert!(message.contains("outside sandbox"), "{message}");
            }
            other => panic!("unexpected error: {other}"),
        }

        // Engines without a filesystem configuration reject native calls.
        let plain = Engine::create_stateless(64, 1).unwrap();
        assert!(!plain.host_filesystem_enabled());
        let error = plain.clone().fs_exists(root.clone()).await.unwrap_err();
        assert!(error.to_string().contains("not configured"), "{error}");
        plain.shutdown().await;

        engine.shutdown().await;
        let closed = engine.clone().fs_exists(root).await.unwrap_err();
        assert!(closed.to_string().contains("runtime is Shutdown"), "{closed}");
    }

    async fn run_to_completion(engine: &Arc<Engine>, code: &str, heap: Option<String>) -> ExecutionInfo {
        let id = engine
            .submit_execution(ExecutionRequest {
                code: code.to_string(),
                file: None,
                heap,
                fs: None,
                session: Some("pi-session".to_string()),
                heap_memory_max_mb: None,
                execution_timeout_secs: None,
                tags: None,
                mcp_headers: None,
            })
            .await
            .unwrap();
        let info = engine.clone().await_execution(id).await.unwrap();
        assert_eq!(info.status, "completed", "{:?}", info.error);
        info
    }

    fn output_of(engine: &Arc<Engine>, id: &str) -> String {
        engine
            .get_execution_output(id.to_string(), None, Some(u64::MAX), None, None)
            .unwrap()
            .data
            .trim()
            .to_string()
    }

    #[test]
    fn builders_report_the_first_missing_required_field() {
        match EngineConfigBuilder::new().build().unwrap_err() {
            RuntimeError::MissingRequiredField { record_type, field, message } => {
                assert_eq!(record_type, "EngineConfig");
                assert_eq!(field, "limits");
                assert_eq!(message, "EngineConfig is missing required field limits");
            }
            other => panic!("unexpected error: {other}"),
        }
        let limits = ExecutionLimitsBuilder::new()
            .heap_memory_max_mb(64)
            .execution_timeout_secs(5)
            .build()
            .unwrap();
        assert_eq!(limits.max_concurrent_executions, None);
        let bucketless = EngineConfigBuilder::new()
            .limits(limits.clone())
            .heap_store(BlobStoreBuilder::new().backend(StoreBackend::S3).build().unwrap())
            .build()
            .unwrap();
        assert!(matches!(Engine::create(bucketless), Err(RuntimeError::InvalidConfig { .. })));
        let empty_authority = EngineConfigBuilder::new()
            .limits(limits)
            .filesystem(FilesystemAccessBuilder::new().policies_json("{}".into()).build().unwrap())
            .build()
            .unwrap();
        let error = match Engine::create(empty_authority) {
            Ok(_) => panic!("empty filesystem authority must be rejected"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("must declare"), "{error}");
    }

    #[tokio::test]
    async fn builder_config_persists_heaps_across_engines_in_a_data_dir() {
        let dir = tempfile::tempdir().unwrap();
        let policy = dir.path().join("policy.rego");
        std::fs::write(&policy, "package mcp.filesystem\ndefault allow = false\n").unwrap();
        let config = EngineConfigBuilder::new()
            .limits(
                ExecutionLimitsBuilder::new()
                    .heap_memory_max_mb(64)
                    .execution_timeout_secs(5)
                    .build()
                    .unwrap(),
            )
            .data_dir(dir.path().to_str().unwrap().to_string())
            .filesystem(
                FilesystemAccessBuilder::new()
                    .policies_json(
                        serde_json::json!({"policies": [{"url": format!("file://{}", policy.display())}]})
                            .to_string(),
                    )
                    .build()
                    .unwrap(),
            )
            .heap_store(BlobStoreBuilder::new().backend(StoreBackend::Directory).build().unwrap())
            .build()
            .unwrap();

        let engine = Engine::create(config.clone()).unwrap();
        let capabilities = engine.capabilities();
        assert!(capabilities.heap && capabilities.sessions && !capabilities.filesystem);
        assert!(engine.host_filesystem_enabled());
        assert!(matches!(engine.mode(), RuntimeMode::LocalStateful));
        let first = run_to_completion(&engine, "globalThis.counter = 41;", None).await;
        let heap = first.heap.expect("a stateful execution reports its heap");
        assert!(dir.path().join("heaps").is_dir());
        engine.shutdown().await;

        // While the first engine still holds the data directory, a second one
        // fails instead of silently running without persistence.
        let error = match Engine::create(config.clone()) {
            Ok(_) => panic!("a data directory in use must be rejected"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("another engine"), "{error}");
        drop(engine);

        // A second engine over the same data directory resumes the heap by hash.
        let engine = Engine::create(config).unwrap();
        let second = run_to_completion(&engine, "console.log(++globalThis.counter)", Some(heap.clone())).await;
        assert_eq!(output_of(&engine, &second.id), "42");
        assert_ne!(second.heap.as_deref(), Some(heap.as_str()));
        let snapshots = engine.list_session_snapshots("pi-session".to_string()).await.unwrap();
        assert_eq!(snapshots.len(), 2);
        engine.shutdown().await;
    }

    fn snapshot_engine(dir: &std::path::Path, heap: bool) -> Arc<Engine> {
        let policy = dir.join("policy.rego");
        std::fs::write(
            &policy,
            "package mcp.filesystem\ndefault allow = false\nallow if { startswith(input.path, \"/work\") }\n",
        )
        .unwrap();
        let mut builder = EngineConfigBuilder::new()
            .limits(
                ExecutionLimitsBuilder::new()
                    .heap_memory_max_mb(64)
                    .execution_timeout_secs(5)
                    .build()
                    .unwrap(),
            )
            .data_dir(dir.to_str().unwrap().to_string())
            .filesystem(
                FilesystemAccessBuilder::new()
                    .policies_json(
                        serde_json::json!({"policies": [{"url": format!("file://{}", policy.display())}]})
                            .to_string(),
                    )
                    .build()
                    .unwrap(),
            )
            .fs_snapshot_store(BlobStoreBuilder::new().backend(StoreBackend::Directory).build().unwrap());
        if heap {
            builder = builder.heap_store(BlobStoreBuilder::new().backend(StoreBackend::Directory).build().unwrap());
        }
        Engine::create(builder.build().unwrap()).unwrap()
    }

    /// Stateful engines answer `run_js` with an execution id; await it and
    /// return the console output, the way an embedding host does.
    async fn run_js_in_session(engine: &Arc<Engine>, session: &str, code: &str) -> String {
        let result = engine
            .clone()
            .call_tool_async(
                "run_js".to_string(),
                serde_json::json!({ "code": code }).to_string(),
                Some(session.to_string()),
                None,
            )
            .await
            .unwrap();
        let value: Value = serde_json::from_str(&result).unwrap();
        let id = value["execution_id"].as_str().unwrap_or_else(|| panic!("{value}")).to_string();
        let info = engine.clone().await_execution(id.clone()).await.unwrap();
        assert_eq!(info.status, "completed", "{:?}", info.error);
        output_of(engine, &id)
    }

    #[tokio::test]
    async fn session_file_views_share_the_snapshot_with_guest_runs() {
        let dir = tempfile::tempdir().unwrap();
        let engine = snapshot_engine(dir.path(), true);
        assert!(engine.capabilities().filesystem && engine.capabilities().heap);

        // A native write lands in the session's snapshot; the next run mounts it.
        let view = engine.clone().fs_view(Some("s1".to_string())).unwrap();
        assert_eq!(view.session().as_deref(), Some("s1"));
        view.clone().write_file("/work/a.txt".into(), b"hello".to_vec()).await.unwrap();
        assert_eq!(view.clone().read_text_file("/work/a.txt".into()).await.unwrap(), "hello");
        let guest = run_js_in_session(
            &engine,
            "s1",
            "globalThis.seen = await fs.readFile('/work/a.txt', 'utf8'); console.log(seen)",
        )
        .await;
        assert_eq!(guest, "hello");

        // A guest write lands in the same snapshot; the next native read sees it,
        // and the session's heap survives the native mutation in between.
        run_js_in_session(&engine, "s1", "await fs.writeFile('/work/b.txt', 'from guest')").await;
        view.clone().append_file("/work/a.txt".into(), b" world".to_vec()).await.unwrap();
        let mut names = view.clone().read_dir("/work".into()).await.unwrap();
        names.sort();
        assert_eq!(names, vec!["a.txt".to_string(), "b.txt".to_string()]);
        assert_eq!(view.clone().read_text_file("/work/b.txt".into()).await.unwrap(), "from guest");
        let stat = view.clone().stat("/work/a.txt".into()).await.unwrap();
        assert_eq!(stat.kind, FsEntryKind::File);
        assert_eq!(stat.size, "hello world".len() as u64);
        let heap_check = run_js_in_session(&engine, "s1", "console.log(globalThis.seen)").await;
        assert_eq!(heap_check, "hello");

        // A no-op mutation adds no log entry; real ones do, carrying the heap forward.
        view.clone().make_dir("/work/empty".into(), true).await.unwrap();
        let snapshots = engine.list_session_snapshots("s1".to_string()).await.unwrap();
        let codes: Vec<&str> = snapshots.iter().map(|entry| entry.code.as_str()).collect();
        assert_eq!(snapshots.len(), 5, "{codes:?}");
        assert!(codes[0].starts_with("// native fs.writeFile"), "{codes:?}");
        assert!(snapshots.iter().all(|entry| entry.output_fs.is_some()));
        assert_eq!(snapshots[3].output_heap, snapshots[2].output_heap, "native mutation keeps the heap");

        // The policy applies to the snapshot namespace too, and views are explicit
        // about which namespace they address.
        let denied = view.clone().read_file("/etc/hostname".into()).await.unwrap_err();
        assert!(matches!(denied, RuntimeError::FileSystem { kind: FsErrorKind::PermissionDenied, .. }), "{denied}");
        let missing = view.clone().read_file("/work/missing".into()).await.unwrap_err();
        assert!(matches!(missing, RuntimeError::FileSystem { kind: FsErrorKind::NotFound, .. }), "{missing}");
        let host = engine.clone().fs_view(None).unwrap();
        assert!(host.session().is_none());
        assert!(!host.exists("/work/a.txt".into()).await.unwrap(), "host view addresses the host filesystem");
        assert!(engine.clone().fs_view(Some(String::new())).is_err());
        engine.shutdown().await;

        // Without a snapshot store, only the host view exists.
        let policies = serde_json::json!({"policies": [{"url": format!("file://{}", dir.path().join("policy.rego").display())}]});
        let stateless = Engine::create_with_filesystem(64, 1, policies.to_string()).unwrap();
        assert!(stateless.clone().fs_view(Some("s1".to_string())).is_err());
        assert!(stateless.clone().fs_view(None).is_ok());
        stateless.shutdown().await;
    }

    #[test]
    fn wasm_modules_are_engine_level_and_exclusive_with_heaps() {
        let dir = tempfile::tempdir().unwrap();
        let module = dir.path().join("empty.wasm");
        std::fs::write(&module, b"\0asm\x01\0\0\0").unwrap();
        let limits = ExecutionLimitsBuilder::new()
            .heap_memory_max_mb(64)
            .execution_timeout_secs(5)
            .build()
            .unwrap();
        let wasm = WasmModuleFileBuilder::new()
            .name("empty".into())
            .path(module.to_str().unwrap().to_string())
            .build()
            .unwrap();
        let conflict = EngineConfigBuilder::new()
            .limits(limits.clone())
            .heap_store(BlobStoreBuilder::new().backend(StoreBackend::Directory).build().unwrap())
            .wasm_modules(vec![wasm.clone()])
            .build()
            .unwrap();
        match Engine::create(conflict) {
            Err(RuntimeError::InvalidConfig { message }) => assert!(message.contains("heap"), "{message}"),
            Err(other) => panic!("unexpected error: {other}"),
            Ok(_) => panic!("heap store and wasm modules must be rejected together"),
        }
        let missing = EngineConfigBuilder::new()
            .limits(limits.clone())
            .wasm_modules(vec![WasmModuleFileBuilder::new()
                .name("nope".into())
                .path(dir.path().join("nope.wasm").to_str().unwrap().to_string())
                .build()
                .unwrap()])
            .build()
            .unwrap();
        assert!(matches!(Engine::create(missing), Err(RuntimeError::InvalidConfig { .. })));
        let engine = Engine::create(
            EngineConfigBuilder::new()
                .limits(limits)
                .wasm_modules(vec![wasm])
                .build()
                .unwrap(),
        )
        .unwrap();
        assert!(!engine.capabilities().heap);
        let result = engine.call_tool(
            "run_js".to_string(),
            r#"{"code":"console.log(typeof empty)"}"#.to_string(),
            None,
            None,
        );
        let value: Value = serde_json::from_str(&result.unwrap()).unwrap();
        assert_eq!(value["output"].as_str().unwrap().trim(), "object", "{value}");
        engine.close().unwrap();
    }

    #[tokio::test]
    async fn lifecycle_shutdown_is_idempotent_and_rejects_new_work() {
        let engine = Engine::from_engine(Engine::new_stateless(64 * 1024 * 1024, 30, 2));
        assert_eq!(engine.lifecycle_state(), RuntimeLifecycleState::Running);

        let first = engine.shutdown().await;
        assert!(!first.already_shutdown);
        assert_eq!(engine.lifecycle_state(), RuntimeLifecycleState::Shutdown);

        let second = engine.shutdown().await;
        assert!(second.already_shutdown);
        let error = engine
            .invoke_tool(ToolCallRequest {
                name: "run_js".to_string(),
                arguments_json: r#"{"code":"console.log(1)"}"#.to_string(),
                session_id: None,
                mcp_headers: None,
            })
            .await
            .unwrap_err();
        assert!(error.to_string().contains("runtime is Shutdown"));
    }
}

#[cfg(test)]
mod embedded_python_tests {
    use super::*;

    #[test]
    fn synchronous_engine_executes_and_closes() {
        let engine = Engine::create_stateless(64, 1).unwrap();
        let run = |code: &str| -> Value {
            serde_json::from_str(
                &engine
                    .call_tool(
                        "run_js".into(),
                        json!({"code": code}).to_string(),
                        None,
                        None,
                    )
                    .unwrap(),
            )
            .unwrap()
        };
        assert!(
            run("console.log(6 * 7)")["output"]
                .as_str()
                .unwrap()
                .contains("42")
        );
        assert!(run("while (true) {}")["error"].is_string());
        assert!(
            run("console.log('again')")["output"]
                .as_str()
                .unwrap()
                .contains("again")
        );
        assert!(!engine.close().unwrap().already_shutdown);
        assert!(engine.close().unwrap().already_shutdown);
        assert!(
            engine
                .call_tool("run_js".into(), json!({"code":"1"}).to_string(), None, None)
                .is_err()
        );
    }

    #[test]
    fn rejects_invalid_limits() {
        for (memory, timeout) in [(0, 1), (15, 1), (4097, 1), (64, 0), (64, 301)] {
            assert!(Engine::create_stateless(memory, timeout).is_err());
        }
    }

    #[tokio::test]
    async fn owned_runtime_can_drop_inside_tokio() {
        let engine = Engine::create_stateless(64, 1).unwrap();
        assert!(engine.close().is_err());
        assert!(
            engine
                .call_tool("run_js".into(), "{}".into(), None, None)
                .is_err()
        );
        drop(engine);
    }
}
