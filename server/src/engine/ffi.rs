//! The engine's FFI surface: uniffi-exported records, error type, factory
//! functions, and the exported `impl Engine` block, plus the builder used by
//! Rust hosts. A child module of `engine`, so it works on the engine's own
//! fields directly — there is no wrapper type and no delegation layer.
//!
//! Everything here is re-exported from `crate::engine`.

use super::*;

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
            Self::InvalidJson { message, .. } | Self::FileSystem { message, .. } => message,
        }
    }
}

#[uniffi::export]
impl Engine {
    /// Enable hook/policy-gated host filesystem access without enabling subprocesses.
    /// The JSON is one `OperationPolicies` object (the `filesystem` entry of
    /// `--policies-json`), so `policies`, `pre`, `post`, and `stack` are all
    /// interpreted exactly as the server interprets them.
    #[uniffi::constructor]
    pub fn create_with_filesystem(
        heap_memory_max_mb: u64,
        execution_timeout_secs: u64,
        filesystem_policy_json: String,
    ) -> Result<Arc<Self>, RuntimeError> {
        let config: opa::OperationPolicies = serde_json::from_str(&filesystem_policy_json)
            .map_err(|error| RuntimeError::InvalidConfig { message: error.to_string() })?;
        // An empty chain permits everything: require explicitly configured authority.
        if config.policies.is_empty() && config.pre.is_empty() && config.stack.is_empty() {
            return Err(RuntimeError::InvalidConfig {
                message: "filesystem configuration must declare policies, pre hooks, or a stack".into(),
            });
        }
        let chain = hooks::build_hook_chain(
            "filesystem",
            &config,
            "mcp/filesystem",
            "data.mcp.filesystem.allow",
            fs::HOOK_CAPS,
        )
        .map_err(|message| RuntimeError::InvalidConfig {
            message: format!("failed to build filesystem hook chain: {message}"),
        })?;
        let mut engine = Self::create_stateless(heap_memory_max_mb, execution_timeout_secs)?;
        Arc::get_mut(&mut engine)
            .ok_or_else(|| RuntimeError::Initialization {
                message: "new embedded engine unexpectedly shared".into(),
            })?
            .fs_config = Some(Arc::new(fs::FsConfig::new_with_hooks(Arc::new(chain))));
        Ok(engine)
    }

    /// True when this engine was constructed with a filesystem hook chain, so
    /// both guest `fs.*` calls and the native `fs_*` methods are available.
    pub fn host_filesystem_enabled(&self) -> bool {
        self.fs_config.is_some()
    }

    /// Read a file as bytes through the filesystem hook chain.
    pub async fn fs_read_file(self: Arc<Self>, path: String) -> Result<Vec<u8>, RuntimeError> {
        self.native_fs(|fs| async move { fs.read_file(&path).await }).await
    }

    /// Read at most `max_bytes` bytes of a file starting at `offset`; fewer
    /// bytes are returned only at end of file. Gated as a read of the file.
    pub async fn fs_read_file_range(
        self: Arc<Self>,
        path: String,
        offset: u64,
        max_bytes: u64,
    ) -> Result<Vec<u8>, RuntimeError> {
        self.native_fs(|fs| async move { fs.read_range(&path, offset, max_bytes).await }).await
    }

    /// The canonical path of an existing path, resolving symlinks. Gated as a
    /// `stat` of the path.
    pub async fn fs_canonical_path(self: Arc<Self>, path: String) -> Result<String, RuntimeError> {
        self.native_fs(|fs| async move { fs.canonical_path(&path).await }).await
    }

    /// Read a file as UTF-8 text; invalid UTF-8 is an `InvalidData` failure.
    pub async fn fs_read_text_file(self: Arc<Self>, path: String) -> Result<String, RuntimeError> {
        self.native_fs(|fs| async move { fs.read_text(&path).await }).await
    }

    /// Create or replace a file with `data`.
    pub async fn fs_write_file(self: Arc<Self>, path: String, data: Vec<u8>) -> Result<(), RuntimeError> {
        self.native_fs(|fs| async move { fs.write_file(&path, &data).await }).await
    }

    /// Append `data` to a file, creating it when missing.
    pub async fn fs_append_file(self: Arc<Self>, path: String, data: Vec<u8>) -> Result<(), RuntimeError> {
        self.native_fs(|fs| async move { fs.append_file(&path, &data).await }).await
    }

    /// Metadata for a path, following a final symlink (Node `fs.stat`).
    pub async fn fs_stat(self: Arc<Self>, path: String) -> Result<FsMetadata, RuntimeError> {
        self.native_fs(|fs| async move { fs.stat(&path, true).await.map(FsMetadata::from) }).await
    }

    /// Metadata for a path without following a final symlink (Node `fs.lstat`).
    pub async fn fs_lstat(self: Arc<Self>, path: String) -> Result<FsMetadata, RuntimeError> {
        self.native_fs(|fs| async move { fs.stat(&path, false).await.map(FsMetadata::from) }).await
    }

    /// Names of a directory's direct children.
    pub async fn fs_read_dir(self: Arc<Self>, path: String) -> Result<Vec<String>, RuntimeError> {
        self.native_fs(|fs| async move { fs.readdir(&path).await }).await
    }

    /// The target of a symlink.
    pub async fn fs_read_link(self: Arc<Self>, path: String) -> Result<String, RuntimeError> {
        self.native_fs(|fs| async move { fs.readlink(&path).await }).await
    }

    /// Create a directory, and its missing parents when `recursive` is set.
    pub async fn fs_make_dir(self: Arc<Self>, path: String, recursive: bool) -> Result<(), RuntimeError> {
        self.native_fs(|fs| async move { fs.mkdir(&path, recursive).await }).await
    }

    /// Remove a file, or a directory (its contents too when `recursive` is set).
    pub async fn fs_remove(self: Arc<Self>, path: String, recursive: bool) -> Result<(), RuntimeError> {
        self.native_fs(|fs| async move { fs.rm(&path, recursive).await }).await
    }

    /// Rename `from` to `to`, replacing an existing destination file.
    pub async fn fs_rename(self: Arc<Self>, from: String, to: String) -> Result<(), RuntimeError> {
        self.native_fs(|fs| async move { fs.rename(&from, &to).await }).await
    }

    /// Whether a path exists. Only the hook chain can fail this call.
    pub async fn fs_exists(self: Arc<Self>, path: String) -> Result<bool, RuntimeError> {
        self.native_fs(|fs| async move { fs.exists(&path).await }).await
    }

    /// Construct a local, capability-restricted engine for synchronous foreign callers.
    #[uniffi::constructor]
    pub fn create_stateless(
        heap_memory_max_mb: u64,
        execution_timeout_secs: u64,
    ) -> Result<Arc<Self>, RuntimeError> {
        if !(16..=4096).contains(&heap_memory_max_mb)
            || !(1..=300).contains(&execution_timeout_secs)
        {
            return Err(RuntimeError::InvalidConfig {
                message: "heap_memory_max_mb must be 16..=4096 and execution_timeout_secs must be 1..=300".into(),
            });
        }
        let bytes = usize::try_from(heap_memory_max_mb * 1024 * 1024).map_err(|_| {
            RuntimeError::InvalidConfig {
                message: "heap limit exceeds platform capacity".into(),
            }
        })?;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|e| RuntimeError::Initialization {
                message: e.to_string(),
            })?;
        let directory = tempfile::tempdir().map_err(|e| RuntimeError::Initialization {
            message: e.to_string(),
        })?;
        initialize_v8();
        let registry =
            ExecutionRegistry::new(directory.path().join("executions").to_str().ok_or_else(
                || RuntimeError::Initialization {
                    message: "temporary path is not UTF-8".into(),
                },
            )?)
            .map_err(|message| RuntimeError::Initialization { message })?;
        let engine = Engine::new_stateless(bytes, execution_timeout_secs, 1)
            .with_execution_registry(Arc::new(registry));
        Ok(Self::wrap(engine, Some(runtime), Some(directory), None))
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

        let mut execution = self
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

    /// The filesystem service native callers share with guest `fs.*` calls:
    /// the same hook chain, headers, and host backend.
    ///
    /// Overlay-backed engines are rejected: the CAS overlay must run on the
    /// current-thread isolate runtime, which foreign callers do not have.
    fn native_fs_service(&self) -> Result<fs::FsService, RuntimeError> {
        let config = self.fs_config.as_ref().ok_or_else(|| RuntimeError::Operation {
            message: "filesystem access is not configured; construct the engine with create_with_filesystem"
                .into(),
        })?;
        if self.fs_store.is_some() {
            return Err(RuntimeError::Operation {
                message: "native filesystem calls are not supported on overlay-backed engines".into(),
            });
        }
        Ok(fs::FsService::host((**config).clone()))
    }

    /// Run one native filesystem operation, on the caller's runtime when there
    /// is one and otherwise on the library-created runtime, without blocking the
    /// foreign host thread. Shutdown waits for in-flight operations, as it does
    /// for tool calls.
    async fn native_fs<T, F, Fut>(self: Arc<Self>, op: F) -> Result<T, RuntimeError>
    where
        T: Send + 'static,
        F: FnOnce(fs::FsService) -> Fut,
        Fut: std::future::Future<Output = Result<T, fs::FsError>> + Send + 'static,
    {
        let _lifecycle_guard = self.shutdown_lock.lock().await;
        self.ensure_running()?;
        let operation = op(self.native_fs_service()?);
        if tokio::runtime::Handle::try_current().is_ok() {
            return operation.await.map_err(RuntimeError::from);
        }
        let tokio_runtime = self
            .tokio_runtime
            .as_ref()
            .ok_or_else(|| RuntimeError::Initialization {
                message: "native filesystem calls require an active or library-created runtime"
                    .to_string(),
            })?;
        tokio_runtime
            .spawn(operation)
            .await
            .map_err(|error| RuntimeError::Operation {
                message: format!("native filesystem task failed: {error}"),
            })?
            .map_err(RuntimeError::from)
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

#[cfg(test)]
mod tests {
    use super::*;

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
