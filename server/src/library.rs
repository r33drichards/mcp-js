use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use crate::cluster::ClusterNode;
use crate::engine::execution::{
    ConsoleOutputPage as EngineConsoleOutputPage, ExecutionInfo as EngineExecutionInfo,
    ExecutionSummary as EngineExecutionSummary,
};
use crate::engine::{
    Engine, FsLabelView, FsMergeConflictView, FsMergeResult, FsPushOutcome, FsRefLogView,
    RunJsRequest, initialize_v8,
};
use crate::runtime::McpJsRuntime;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const DEFAULT_HEAP_MEMORY_MB: u64 = 64;
const DEFAULT_EXECUTION_TIMEOUT_SECS: u64 = 30;
const DEFAULT_MAX_CONCURRENT_EXECUTIONS: u32 = 4;

#[derive(Clone, Copy, Debug, uniffi::Enum)]
pub enum LibraryMode {
    Stateless,
    LocalStateful,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum LibraryLifecycleState {
    Running,
    ShuttingDown,
    Shutdown,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct LibraryShutdownResult {
    pub cancelled_executions: u64,
    pub closed_mcp_connections: u64,
    pub cluster_shutdown: bool,
    pub already_shutdown: bool,
}

#[derive(Clone, Debug, Default, uniffi::Record)]
pub struct LibraryHardeningConfig {
    pub freeze_ops: bool,
    pub neutralize_proxy_details: bool,
    pub neutralize_introspection: bool,
    pub remove_bootstrap: bool,
    pub remove_shared_memory: bool,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct LibraryWasmModuleConfig {
    pub name: String,
    pub bytes: Vec<u8>,
    pub max_memory_bytes: Option<u64>,
    pub description: Option<String>,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct LibraryWasmStubConfig {
    pub prefix: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct LibraryFeatureConfig {
    pub wasm_default_max_bytes: u64,
    pub hardening: LibraryHardeningConfig,
    pub wasm_modules: Vec<LibraryWasmModuleConfig>,
    pub wasm_stubs: LibraryWasmStubConfig,
    pub instructions_override: Option<String>,
    pub run_js_description_override: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, uniffi::Enum)]
#[serde(rename_all = "lowercase")]
pub enum LibraryPolicyEvalMode {
    #[default]
    All,
    Any,
}

#[derive(Clone, Debug, Deserialize, uniffi::Record)]
pub struct LibraryPolicySource {
    pub url: String,
    pub policy_path: Option<String>,
    pub rule: Option<String>,
}

#[derive(Clone, Debug, Deserialize, uniffi::Record)]
pub struct LibraryOperationPolicies {
    #[serde(default)]
    pub mode: LibraryPolicyEvalMode,
    pub policies: Vec<LibraryPolicySource>,
}

#[derive(Clone, Debug, Default, Deserialize, uniffi::Record)]
pub struct LibraryPolicyConfig {
    pub fetch: Option<LibraryOperationPolicies>,
    pub modules: Option<LibraryOperationPolicies>,
    pub filesystem: Option<LibraryOperationPolicies>,
    pub fs_snapshot: Option<LibraryOperationPolicies>,
    pub mcp_tools: Option<LibraryOperationPolicies>,
    pub subprocess: Option<LibraryOperationPolicies>,
    pub run_js_file: Option<LibraryOperationPolicies>,
}

#[derive(Clone, uniffi::Record)]
pub struct LibraryFetchOAuthConfig {
    pub header_name: String,
    pub token_url: String,
    pub client_id: String,
    pub client_secret: String,
    pub scope: Option<String>,
    pub refresh_buffer_secs: u64,
}

impl std::fmt::Debug for LibraryFetchOAuthConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LibraryFetchOAuthConfig")
            .field("header_name", &self.header_name)
            .field("token_url", &self.token_url)
            .field("client_id", &self.client_id)
            .field("client_secret", &"<redacted>")
            .field("scope", &self.scope)
            .field("refresh_buffer_secs", &self.refresh_buffer_secs)
            .finish()
    }
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct LibraryFetchHeaderRule {
    pub host: String,
    pub methods: Vec<String>,
    pub static_headers: Option<HashMap<String, String>>,
    pub oauth: Option<LibraryFetchOAuthConfig>,
}

impl LibraryFetchHeaderRule {
    pub fn validate(&self) -> Result<(), LibraryError> {
        crate::bootstrap::validate_fetch_header_rule(self)
    }

    pub fn normalized(self) -> Result<Self, LibraryError> {
        crate::bootstrap::normalize_fetch_header_rule(self)
    }

    pub fn methods(&self) -> &[String] {
        &self.methods
    }

    pub fn static_headers(&self) -> Option<&HashMap<String, String>> {
        self.static_headers.as_ref()
    }

    pub fn dynamic_auth(&self) -> Option<&LibraryFetchOAuthConfig> {
        self.oauth.as_ref()
    }
}

#[derive(Clone, Copy, Debug, Default, uniffi::Enum)]
pub enum LibraryRunJsFileAccess {
    #[default]
    Disabled,
    AllowAll,
    Policy,
}

#[derive(Clone, Debug, Default, uniffi::Record)]
pub struct LibraryCapabilityConfig {
    pub fetch_header_rules: Vec<LibraryFetchHeaderRule>,
    pub filesystem_passthrough: bool,
    pub allow_external_modules: bool,
    pub run_js_file_access: LibraryRunJsFileAccess,
}

#[derive(Clone, Copy, Debug, uniffi::Enum)]
pub enum LibraryMcpTransportKind {
    Stdio,
    Sse,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct LibraryMcpServerConfig {
    pub name: String,
    pub transport: LibraryMcpTransportKind,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub url: Option<String>,
}

impl<'de> Deserialize<'de> for LibraryMcpServerConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(tag = "transport", rename_all = "lowercase")]
        enum Transport {
            Stdio {
                command: String,
                #[serde(default)]
                args: Vec<String>,
                #[serde(default)]
                env: HashMap<String, String>,
            },
            Sse {
                url: String,
            },
        }

        #[derive(Deserialize)]
        struct Config {
            name: String,
            #[serde(flatten)]
            transport: Transport,
        }

        let config = Config::deserialize(deserializer)?;
        Ok(match config.transport {
            Transport::Stdio { command, args, env } => Self {
                name: config.name,
                transport: LibraryMcpTransportKind::Stdio,
                command: Some(command),
                args,
                env,
                url: None,
            },
            Transport::Sse { url } => Self {
                name: config.name,
                transport: LibraryMcpTransportKind::Sse,
                command: None,
                args: Vec::new(),
                env: HashMap::new(),
                url: Some(url),
            },
        })
    }
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct LibraryMcpStubConfig {
    pub prefix: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct LibraryUpstreamMcpConfig {
    pub servers: Vec<LibraryMcpServerConfig>,
    pub stubs: LibraryMcpStubConfig,
}

#[derive(Clone, Copy, Debug, uniffi::Enum)]
pub enum LibraryStorageKind {
    None,
    Directory,
    S3,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct LibraryRuntimeConfig {
    pub heap_store: LibraryStorageKind,
    pub heap_dir: Option<String>,
    pub filesystem_store: LibraryStorageKind,
    pub filesystem_dir: Option<String>,
    pub filesystem_labels_db: Option<String>,
    pub s3_bucket: Option<String>,
    pub cache_dir: Option<String>,
    pub session_db_path: String,
    pub execution_db_path: Option<String>,
    pub heap_memory_max_mb: u64,
    pub execution_timeout_secs: u64,
    pub max_concurrent_executions: u32,
    pub session_id: Option<String>,
    pub session_fork_from: Option<String>,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct LibraryConfig {
    pub mode: LibraryMode,
    pub data_dir: Option<String>,
    pub heap_memory_max_mb: u64,
    pub execution_timeout_secs: u64,
    pub max_concurrent_executions: u32,
    pub filesystem_enabled: bool,
}

impl Default for LibraryConfig {
    fn default() -> Self {
        Self {
            mode: LibraryMode::Stateless,
            data_dir: None,
            heap_memory_max_mb: DEFAULT_HEAP_MEMORY_MB,
            execution_timeout_secs: DEFAULT_EXECUTION_TIMEOUT_SECS,
            max_concurrent_executions: DEFAULT_MAX_CONCURRENT_EXECUTIONS,
            filesystem_enabled: false,
        }
    }
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct ToolDefinition {
    pub name: String,
    pub description: Option<String>,
    pub input_schema_json: String,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct LibraryMcpRequestHeaders {
    pub values: HashMap<String, String>,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct LibraryToolCallRequest {
    pub name: String,
    pub arguments_json: String,
    pub session_id: Option<String>,
    pub mcp_headers: Option<LibraryMcpRequestHeaders>,
}

#[derive(Clone, Debug, Serialize, uniffi::Record)]
pub struct LibraryCapabilities {
    pub heap: bool,
    pub filesystem: bool,
    pub sessions: bool,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct LibraryExecutionRequest {
    pub code: String,
    pub file: Option<String>,
    pub heap: Option<String>,
    pub fs: Option<String>,
    pub session: Option<String>,
    pub heap_memory_max_mb: Option<u64>,
    pub execution_timeout_secs: Option<u64>,
    pub tags: Option<HashMap<String, String>>,
    pub mcp_headers: Option<LibraryMcpRequestHeaders>,
}

#[derive(Clone, Debug, Serialize, uniffi::Record)]
pub struct LibraryExecutionInfo {
    pub execution_id: String,
    pub status: String,
    pub result: Option<String>,
    pub heap: Option<String>,
    pub fs: Option<String>,
    pub error: Option<String>,
    pub started_at: String,
    pub completed_at: Option<String>,
}

#[derive(Clone, Debug, Serialize, uniffi::Record)]
pub struct LibraryExecutionSummary {
    pub execution_id: String,
    pub status: String,
    pub started_at: String,
    pub completed_at: Option<String>,
}

#[derive(Clone, Debug, Serialize, uniffi::Record)]
pub struct LibraryExecutionOutput {
    pub data: String,
    pub start_line: u64,
    pub end_line: u64,
    pub next_line_offset: u64,
    pub total_lines: u64,
    pub start_byte: u64,
    pub end_byte: u64,
    pub next_byte_offset: u64,
    pub total_bytes: u64,
    pub has_more: bool,
}

#[derive(Clone, Debug, Serialize, uniffi::Record)]
pub struct LibraryHeapTagEntry {
    pub heap: String,
    pub tags: HashMap<String, String>,
}

#[derive(Clone, Debug, Serialize, uniffi::Record)]
pub struct LibraryFsLabel {
    pub name: String,
    pub ca_id: String,
}

#[derive(Clone, Debug, Serialize, uniffi::Record)]
pub struct LibraryFsRefLogEntry {
    pub at: i64,
    pub from: Option<String>,
    pub to: String,
    pub op: String,
    pub message: Option<String>,
}

#[derive(Clone, Debug, Serialize, uniffi::Record)]
pub struct LibraryFsPushResult {
    pub status: String,
    pub label: String,
    pub ca_id: Option<String>,
    pub current: Option<String>,
}

#[derive(Clone, Debug, Serialize, uniffi::Record)]
pub struct LibraryFsMergeConflict {
    pub path: String,
    pub base: Option<String>,
    pub ours: Option<String>,
    pub theirs: Option<String>,
    pub kind: String,
    pub markers: Option<String>,
    pub diff_ours: Option<String>,
    pub diff_theirs: Option<String>,
}

#[derive(Clone, Debug, Serialize, uniffi::Record)]
pub struct LibraryFsMergeResult {
    pub status: String,
    pub ca_id: Option<String>,
    pub conflicts: Vec<LibraryFsMergeConflict>,
}

#[derive(Clone, Copy, Debug, uniffi::Enum)]
pub enum LibraryFsMergePreference {
    Ours,
    Theirs,
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum LibraryError {
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
}

#[derive(uniffi::Object)]
pub struct McpJsLibrary {
    tokio_runtime: Option<tokio::runtime::Runtime>,
    runtime: McpJsRuntime,
    cluster_node: Option<Arc<ClusterNode>>,
    lifecycle: AtomicU8,
    shutdown_lock: tokio::sync::Mutex<()>,
    _ephemeral_data_dir: Option<tempfile::TempDir>,
}

#[uniffi::export]
pub fn default_library_config() -> LibraryConfig {
    LibraryConfig::default()
}

#[uniffi::export]
pub fn default_feature_config() -> LibraryFeatureConfig {
    LibraryFeatureConfig {
        wasm_default_max_bytes: crate::engine::DEFAULT_WASM_MAX_BYTES as u64,
        hardening: LibraryHardeningConfig::default(),
        wasm_modules: Vec::new(),
        wasm_stubs: LibraryWasmStubConfig {
            prefix: crate::engine::wasm_stub::DEFAULT_WASM_STUB_PREFIX.to_string(),
            enabled: true,
        },
        instructions_override: None,
        run_js_description_override: None,
    }
}

#[uniffi::export]
pub fn default_policy_config() -> LibraryPolicyConfig {
    LibraryPolicyConfig::default()
}

#[uniffi::export]
pub fn default_capability_config() -> LibraryCapabilityConfig {
    LibraryCapabilityConfig::default()
}

#[uniffi::export]
pub fn default_upstream_mcp_config() -> LibraryUpstreamMcpConfig {
    LibraryUpstreamMcpConfig {
        servers: Vec::new(),
        stubs: LibraryMcpStubConfig {
            prefix: crate::engine::mcp_client::DEFAULT_STUB_PREFIX.to_string(),
            enabled: true,
        },
    }
}

#[uniffi::export]
pub fn default_runtime_config(data_dir: String) -> LibraryRuntimeConfig {
    LibraryRuntimeConfig {
        heap_store: LibraryStorageKind::None,
        heap_dir: None,
        filesystem_store: LibraryStorageKind::None,
        filesystem_dir: None,
        filesystem_labels_db: None,
        s3_bucket: None,
        cache_dir: None,
        session_db_path: data_dir,
        execution_db_path: None,
        heap_memory_max_mb: DEFAULT_HEAP_MEMORY_MB,
        execution_timeout_secs: DEFAULT_EXECUTION_TIMEOUT_SECS,
        max_concurrent_executions: DEFAULT_MAX_CONCURRENT_EXECUTIONS,
        session_id: None,
        session_fork_from: None,
    }
}

#[uniffi::export]
pub fn create_library(config: LibraryRuntimeConfig) -> Result<Arc<McpJsLibrary>, LibraryError> {
    create_library_with_features(config, default_feature_config())
}

#[uniffi::export]
pub fn create_library_with_features(
    config: LibraryRuntimeConfig,
    features: LibraryFeatureConfig,
) -> Result<Arc<McpJsLibrary>, LibraryError> {
    create_library_with_configuration(
        config,
        features,
        default_policy_config(),
        default_capability_config(),
    )
}

#[uniffi::export]
pub fn create_library_with_configuration(
    config: LibraryRuntimeConfig,
    features: LibraryFeatureConfig,
    policies: LibraryPolicyConfig,
    capabilities: LibraryCapabilityConfig,
) -> Result<Arc<McpJsLibrary>, LibraryError> {
    create_library_with_upstreams(
        config,
        features,
        policies,
        capabilities,
        default_upstream_mcp_config(),
    )
}

#[uniffi::export]
pub fn create_library_with_upstreams(
    config: LibraryRuntimeConfig,
    features: LibraryFeatureConfig,
    policies: LibraryPolicyConfig,
    capabilities: LibraryCapabilityConfig,
    upstreams: LibraryUpstreamMcpConfig,
) -> Result<Arc<McpJsLibrary>, LibraryError> {
    validate_runtime_config(&config)?;
    initialize_v8();
    let worker_threads = usize::try_from(config.max_concurrent_executions).map_err(|_| {
        LibraryError::InvalidConfig {
            message: "max_concurrent_executions is too large for this platform".to_string(),
        }
    })?;
    let tokio_runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(worker_threads)
        .build()
        .map_err(|error| LibraryError::Initialization {
            message: error.to_string(),
        })?;
    let bootstrap_config = runtime_bootstrap_config(config)?;
    let bootstrap = tokio_runtime.block_on(async {
        let bootstrap = crate::bootstrap::build_storage_engine(bootstrap_config, None)
            .await
            .map_err(|error| LibraryError::Initialization {
                message: error.to_string(),
            })?
            .with_feature_config(features)?
            .with_policy_config(policies, capabilities)?;
        bootstrap.with_upstream_mcp_config(upstreams).await
    })?;
    Ok(bootstrap.build_with_runtime(tokio_runtime))
}

#[uniffi::export]
impl McpJsLibrary {
    #[uniffi::constructor]
    pub fn new(config: LibraryConfig) -> Result<Arc<Self>, LibraryError> {
        validate_config(&config)?;
        initialize_v8();

        let tokio_runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(config.max_concurrent_executions as usize)
            .build()
            .map_err(|error| LibraryError::Initialization {
                message: error.to_string(),
            })?;

        let (runtime, ephemeral_data_dir) = build_runtime(&config)?;
        Ok(Self::wrap(
            runtime,
            Some(tokio_runtime),
            ephemeral_data_dir,
            None,
        ))
    }

    pub fn mode(&self) -> LibraryMode {
        if self.runtime.session_capable() {
            LibraryMode::LocalStateful
        } else {
            LibraryMode::Stateless
        }
    }

    pub fn lifecycle_state(&self) -> LibraryLifecycleState {
        self.current_lifecycle_state()
    }

    pub async fn shutdown(&self) -> LibraryShutdownResult {
        let _guard = self.shutdown_lock.lock().await;
        if self.current_lifecycle_state() == LibraryLifecycleState::Shutdown {
            return LibraryShutdownResult {
                cancelled_executions: 0,
                closed_mcp_connections: 0,
                cluster_shutdown: false,
                already_shutdown: true,
            };
        }

        self.lifecycle
            .store(LibraryLifecycleState::ShuttingDown as u8, Ordering::Release);
        let (cancelled_executions, closed_mcp_connections) = self.runtime.shutdown().await;
        let cluster_shutdown = self.cluster_node.as_ref().is_some_and(|node| {
            node.shutdown();
            true
        });
        self.lifecycle
            .store(LibraryLifecycleState::Shutdown as u8, Ordering::Release);

        LibraryShutdownResult {
            cancelled_executions,
            closed_mcp_connections,
            cluster_shutdown,
            already_shutdown: false,
        }
    }

    pub fn capabilities(&self) -> LibraryCapabilities {
        LibraryCapabilities {
            heap: self.runtime.heap_enabled(),
            filesystem: self.runtime.fs_enabled(),
            sessions: self.runtime.session_capable(),
        }
    }

    pub async fn submit_execution(
        &self,
        request: LibraryExecutionRequest,
    ) -> Result<String, LibraryError> {
        let _lifecycle_guard = self.shutdown_lock.lock().await;
        self.ensure_running()?;
        if request.code.is_empty() && request.file.is_none() {
            return Err(LibraryError::InvalidConfig {
                message: "execution requires code or a file path".to_string(),
            });
        }
        if !request.code.is_empty() && request.file.is_some() {
            return Err(LibraryError::InvalidConfig {
                message: "execution cannot specify both code and a file path".to_string(),
            });
        }
        let heap_memory_max_mb = request
            .heap_memory_max_mb
            .map(usize::try_from)
            .transpose()
            .map_err(|_| LibraryError::InvalidConfig {
                message: "heap_memory_max_mb is too large for this platform".to_string(),
            })?;
        let mcp_headers = request.mcp_headers.map(library_mcp_headers_value);

        let mut execution = self
            .runtime
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

    pub fn get_execution(
        &self,
        execution_id: String,
    ) -> Result<LibraryExecutionInfo, LibraryError> {
        self.runtime
            .get_execution(&execution_id)
            .map(LibraryExecutionInfo::from)
            .map_err(operation_message)
    }

    pub fn get_execution_output(
        &self,
        execution_id: String,
        line_offset: Option<u64>,
        line_limit: Option<u64>,
        byte_offset: Option<u64>,
        byte_limit: Option<u64>,
    ) -> Result<LibraryExecutionOutput, LibraryError> {
        self.runtime
            .get_execution_output(
                &execution_id,
                line_offset,
                line_limit,
                byte_offset,
                byte_limit,
            )
            .map(LibraryExecutionOutput::from)
            .map_err(operation_message)
    }

    pub fn cancel_execution(&self, execution_id: String) -> Result<(), LibraryError> {
        self.runtime
            .cancel_execution(&execution_id)
            .map_err(operation_message)
    }

    pub fn list_executions(&self) -> Result<Vec<LibraryExecutionSummary>, LibraryError> {
        self.runtime
            .list_executions()
            .map(|executions| {
                executions
                    .into_iter()
                    .map(LibraryExecutionSummary::from)
                    .collect()
            })
            .map_err(operation_message)
    }

    pub async fn list_sessions(&self) -> Result<Vec<String>, LibraryError> {
        self.runtime
            .list_sessions()
            .await
            .map_err(operation_message)
    }

    pub async fn list_session_snapshots(
        &self,
        session: String,
        fields: Option<Vec<String>>,
    ) -> Result<Vec<String>, LibraryError> {
        self.runtime
            .list_session_snapshots(session, fields)
            .await
            .map_err(operation_message)?
            .into_iter()
            .map(|snapshot| {
                serde_json::to_string(&snapshot).map_err(|error| LibraryError::Operation {
                    message: format!("failed to serialize session snapshot: {error}"),
                })
            })
            .collect()
    }

    pub async fn get_heap_tags(
        &self,
        heap: String,
    ) -> Result<HashMap<String, String>, LibraryError> {
        self.runtime
            .get_heap_tags(heap)
            .await
            .map_err(operation_message)
    }

    pub async fn set_heap_tags(
        &self,
        heap: String,
        tags: HashMap<String, String>,
    ) -> Result<(), LibraryError> {
        self.runtime
            .set_heap_tags(heap, tags)
            .await
            .map_err(operation_message)
    }

    pub async fn delete_heap_tags(
        &self,
        heap: String,
        keys: Option<Vec<String>>,
    ) -> Result<(), LibraryError> {
        self.runtime
            .delete_heap_tags(heap, keys)
            .await
            .map_err(operation_message)
    }

    pub async fn query_heaps_by_tags(
        &self,
        tags: HashMap<String, String>,
    ) -> Result<Vec<LibraryHeapTagEntry>, LibraryError> {
        self.runtime
            .query_heaps_by_tags(tags)
            .await
            .map(|entries| {
                entries
                    .into_iter()
                    .map(|entry| LibraryHeapTagEntry {
                        heap: entry.heap,
                        tags: entry.tags,
                    })
                    .collect()
            })
            .map_err(operation_message)
    }

    pub async fn fs_list_labels(&self) -> Result<Vec<LibraryFsLabel>, LibraryError> {
        self.runtime
            .fs_list_labels()
            .await
            .map(|labels| labels.into_iter().map(LibraryFsLabel::from).collect())
            .map_err(operation_message)
    }

    pub async fn fs_resolve_label(&self, name: String) -> Result<Option<String>, LibraryError> {
        self.runtime
            .fs_resolve_label(&name)
            .await
            .map_err(operation_message)
    }

    pub async fn fs_set_label(
        &self,
        name: String,
        ca_id: String,
        message: Option<String>,
    ) -> Result<(), LibraryError> {
        self.runtime
            .fs_set_label(&name, &ca_id, message)
            .await
            .map_err(operation_message)
    }

    pub async fn fs_label_log(
        &self,
        name: String,
        limit: Option<u64>,
    ) -> Result<Vec<LibraryFsRefLogEntry>, LibraryError> {
        let limit =
            limit
                .map(usize::try_from)
                .transpose()
                .map_err(|_| LibraryError::Operation {
                    message: "filesystem log limit is too large for this platform".to_string(),
                })?;
        self.runtime
            .fs_label_log(&name, limit)
            .await
            .map(|entries| {
                entries
                    .into_iter()
                    .map(LibraryFsRefLogEntry::from)
                    .collect()
            })
            .map_err(operation_message)
    }

    pub async fn fs_push(
        &self,
        label: String,
        ca_id: String,
        expected: Option<String>,
        force: bool,
        message: Option<String>,
    ) -> Result<LibraryFsPushResult, LibraryError> {
        self.runtime
            .fs_push(&label, &ca_id, expected, force, message)
            .await
            .map(LibraryFsPushResult::from)
            .map_err(operation_message)
    }

    pub async fn fs_reset(
        &self,
        label: String,
        ca_id: String,
        allow_unlogged: bool,
        message: Option<String>,
    ) -> Result<(), LibraryError> {
        self.runtime
            .fs_reset(&label, &ca_id, allow_unlogged, message)
            .await
            .map_err(operation_message)
    }

    pub async fn fs_merge(
        &self,
        ours: String,
        theirs: String,
        base: Option<String>,
        prefer: Option<LibraryFsMergePreference>,
    ) -> Result<LibraryFsMergeResult, LibraryError> {
        let prefer = match prefer {
            Some(LibraryFsMergePreference::Ours) => crate::engine::fs_merge::Prefer::Ours,
            Some(LibraryFsMergePreference::Theirs) => crate::engine::fs_merge::Prefer::Theirs,
            None => crate::engine::fs_merge::Prefer::None,
        };
        self.runtime
            .fs_merge(&ours, &theirs, base, prefer)
            .await
            .map(LibraryFsMergeResult::from)
            .map_err(operation_message)
    }

    pub fn list_tools(&self) -> Result<Vec<ToolDefinition>, LibraryError> {
        self.mcp_tools()
            .into_iter()
            .map(|tool| {
                let input_schema_json =
                    serde_json::to_string(tool.input_schema.as_ref()).map_err(|error| {
                        LibraryError::Initialization {
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
        mcp_headers: Option<LibraryMcpRequestHeaders>,
    ) -> Result<String, LibraryError> {
        let tokio_runtime =
            self.tokio_runtime
                .as_ref()
                .ok_or_else(|| LibraryError::Initialization {
                    message: "synchronous tool calls require a library-created runtime".to_string(),
                })?;
        tokio_runtime.block_on(self.invoke_tool(LibraryToolCallRequest {
            name,
            arguments_json,
            session_id,
            mcp_headers,
        }))
    }

    pub async fn invoke_tool(
        &self,
        request: LibraryToolCallRequest,
    ) -> Result<String, LibraryError> {
        let _lifecycle_guard = self.shutdown_lock.lock().await;
        self.ensure_running()?;
        let arguments = parse_json_object("arguments_json", &request.arguments_json)?;
        let mcp_headers = request.mcp_headers.map(library_mcp_headers_value);
        let result = self
            .runtime
            .call_tool(
                request.session_id.as_deref(),
                mcp_headers.as_ref(),
                &request.name,
                &arguments,
            )
            .await;

        serde_json::to_string(&result).map_err(|error| LibraryError::ToolCall {
            message: format!("failed to serialize result: {error}"),
        })
    }
}

impl McpJsLibrary {
    /// Wrap a fully configured runtime for Rust transports without creating a
    /// second Tokio executor or crossing the FFI boundary.
    fn wrap(
        runtime: McpJsRuntime,
        tokio_runtime: Option<tokio::runtime::Runtime>,
        ephemeral_data_dir: Option<tempfile::TempDir>,
        cluster_node: Option<Arc<ClusterNode>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            tokio_runtime,
            runtime,
            cluster_node,
            lifecycle: AtomicU8::new(LibraryLifecycleState::Running as u8),
            shutdown_lock: tokio::sync::Mutex::new(()),
            _ephemeral_data_dir: ephemeral_data_dir,
        })
    }

    fn current_lifecycle_state(&self) -> LibraryLifecycleState {
        match self.lifecycle.load(Ordering::Acquire) {
            value if value == LibraryLifecycleState::Running as u8 => {
                LibraryLifecycleState::Running
            }
            value if value == LibraryLifecycleState::ShuttingDown as u8 => {
                LibraryLifecycleState::ShuttingDown
            }
            _ => LibraryLifecycleState::Shutdown,
        }
    }

    fn ensure_running(&self) -> Result<(), LibraryError> {
        match self.current_lifecycle_state() {
            LibraryLifecycleState::Running => Ok(()),
            state => Err(LibraryError::Operation {
                message: format!("library is {state:?}"),
            }),
        }
    }

    pub fn from_runtime(runtime: McpJsRuntime) -> Arc<Self> {
        Self::wrap(runtime, None, None, None)
    }

    pub fn from_engine(engine: Engine) -> Arc<Self> {
        Self::from_engine_with_cluster(engine, None)
    }

    pub(crate) fn from_engine_with_cluster(
        engine: Engine,
        cluster_node: Option<Arc<ClusterNode>>,
    ) -> Arc<Self> {
        Self::wrap(McpJsRuntime::new(engine), None, None, cluster_node)
    }

    pub(crate) fn from_engine_with_tokio_runtime(
        engine: Engine,
        tokio_runtime: tokio::runtime::Runtime,
        cluster_node: Option<Arc<ClusterNode>>,
    ) -> Arc<Self> {
        Self::wrap(
            McpJsRuntime::new(engine),
            Some(tokio_runtime),
            None,
            cluster_node,
        )
    }

    pub fn heap_enabled(&self) -> bool {
        self.runtime.heap_enabled()
    }

    pub fn fs_enabled(&self) -> bool {
        self.runtime.fs_enabled()
    }

    pub fn session_capable(&self) -> bool {
        self.runtime.session_capable()
    }

    pub fn tool_catalog(&self) -> crate::mcp::ToolCatalog {
        self.runtime.tool_catalog()
    }

    pub fn instructions_override(&self) -> Option<Arc<str>> {
        self.runtime.instructions_override()
    }

    pub fn run_js_description_override(&self) -> Option<Arc<str>> {
        self.runtime.run_js_description_override()
    }

    pub fn wasm_stub_tools(&self) -> Vec<rmcp::model::Tool> {
        self.runtime.wasm_stub_tools()
    }

    pub fn core_mcp_tools(&self) -> Vec<rmcp::model::Tool> {
        crate::mcp::mode_tool_list(self)
    }

    pub fn mcp_tools(&self) -> Vec<rmcp::model::Tool> {
        let mut tools = self.core_mcp_tools();
        tools.extend(self.runtime.upstream_mcp_stub_tools());
        tools.extend(self.wasm_stub_tools());
        tools
    }

    pub fn upstream_mcp_stub_call_response(
        &self,
        name: &str,
        arguments: Option<&serde_json::Map<String, Value>>,
    ) -> Option<rmcp::model::CallToolResult> {
        self.runtime
            .upstream_mcp_stub_call_response(name, arguments)
    }

    pub fn wasm_stub_call_response(
        &self,
        name: &str,
        arguments: Option<&serde_json::Map<String, Value>>,
    ) -> Option<rmcp::model::CallToolResult> {
        self.runtime.wasm_stub_call_response(name, arguments)
    }

    pub fn run_js(&self, code: impl Into<String>) -> RunJsRequest<'_> {
        self.runtime.run_js(code)
    }
}

impl From<FsLabelView> for LibraryFsLabel {
    fn from(label: FsLabelView) -> Self {
        Self {
            name: label.name,
            ca_id: label.ca_id,
        }
    }
}

impl From<FsRefLogView> for LibraryFsRefLogEntry {
    fn from(entry: FsRefLogView) -> Self {
        Self {
            at: entry.at,
            from: entry.from,
            to: entry.to,
            op: entry.op,
            message: entry.message,
        }
    }
}

impl From<FsPushOutcome> for LibraryFsPushResult {
    fn from(outcome: FsPushOutcome) -> Self {
        match outcome {
            FsPushOutcome::Advanced { label, ca_id } => Self {
                status: "advanced".to_string(),
                label,
                ca_id: Some(ca_id),
                current: None,
            },
            FsPushOutcome::Rejected { label, current } => Self {
                status: "rejected".to_string(),
                label,
                ca_id: None,
                current,
            },
        }
    }
}

impl From<FsMergeConflictView> for LibraryFsMergeConflict {
    fn from(conflict: FsMergeConflictView) -> Self {
        Self {
            path: conflict.path,
            base: conflict.base,
            ours: conflict.ours,
            theirs: conflict.theirs,
            kind: conflict.kind,
            markers: conflict.markers,
            diff_ours: conflict.diff_ours,
            diff_theirs: conflict.diff_theirs,
        }
    }
}

impl From<FsMergeResult> for LibraryFsMergeResult {
    fn from(result: FsMergeResult) -> Self {
        match result {
            FsMergeResult::Merged { ca_id } => Self {
                status: "merged".to_string(),
                ca_id: Some(ca_id),
                conflicts: Vec::new(),
            },
            FsMergeResult::Conflict { conflicts } => Self {
                status: "conflict".to_string(),
                ca_id: None,
                conflicts: conflicts
                    .into_iter()
                    .map(LibraryFsMergeConflict::from)
                    .collect(),
            },
        }
    }
}

impl From<EngineExecutionInfo> for LibraryExecutionInfo {
    fn from(info: EngineExecutionInfo) -> Self {
        Self {
            execution_id: info.id,
            status: info.status,
            result: info.result,
            heap: info.heap,
            fs: info.fs,
            error: info.error,
            started_at: info.started_at,
            completed_at: info.completed_at,
        }
    }
}

impl From<EngineExecutionSummary> for LibraryExecutionSummary {
    fn from(summary: EngineExecutionSummary) -> Self {
        Self {
            execution_id: summary.id,
            status: summary.status,
            started_at: summary.started_at,
            completed_at: summary.completed_at,
        }
    }
}

impl From<EngineConsoleOutputPage> for LibraryExecutionOutput {
    fn from(page: EngineConsoleOutputPage) -> Self {
        Self {
            data: page.data,
            start_line: page.start_line,
            end_line: page.end_line,
            next_line_offset: page.next_line_offset,
            total_lines: page.total_lines,
            start_byte: page.start_byte,
            end_byte: page.end_byte,
            next_byte_offset: page.next_byte_offset,
            total_bytes: page.total_bytes,
            has_more: page.has_more,
        }
    }
}

fn operation_message(message: String) -> LibraryError {
    LibraryError::Operation { message }
}

fn validate_runtime_config(config: &LibraryRuntimeConfig) -> Result<(), LibraryError> {
    if config.session_db_path.is_empty() {
        return Err(LibraryError::InvalidConfig {
            message: "session_db_path must not be empty".to_string(),
        });
    }
    if config.heap_memory_max_mb < crate::engine::MIN_HEAP_MEMORY_MB as u64 {
        return Err(LibraryError::InvalidConfig {
            message: format!(
                "heap_memory_max_mb must be at least {}",
                crate::engine::MIN_HEAP_MEMORY_MB
            ),
        });
    }
    if config.execution_timeout_secs == 0 || config.max_concurrent_executions == 0 {
        return Err(LibraryError::InvalidConfig {
            message: "execution timeout and concurrency must be greater than zero".to_string(),
        });
    }
    let uses_s3 = matches!(config.heap_store, LibraryStorageKind::S3)
        || matches!(config.filesystem_store, LibraryStorageKind::S3);
    if uses_s3 && config.s3_bucket.is_none() {
        return Err(LibraryError::InvalidConfig {
            message: "S3 storage requires s3_bucket".to_string(),
        });
    }
    Ok(())
}

fn runtime_bootstrap_config(
    config: LibraryRuntimeConfig,
) -> Result<crate::bootstrap::StorageBootstrapConfig, LibraryError> {
    let heap_memory_max_mb =
        usize::try_from(config.heap_memory_max_mb).map_err(|_| LibraryError::InvalidConfig {
            message: "heap_memory_max_mb is too large for this platform".to_string(),
        })?;
    Ok(crate::bootstrap::StorageBootstrapConfig {
        heap_store: storage_kind(config.heap_store),
        heap_dir: config.heap_dir,
        fs_store: storage_kind(config.filesystem_store),
        fs_dir: config.filesystem_dir,
        fs_labels_db: config.filesystem_labels_db,
        s3_bucket: config.s3_bucket,
        cache_dir: config.cache_dir,
        session_db_path: config.session_db_path,
        http_port: None,
        execution_db_path: config.execution_db_path,
        heap_memory_max_bytes: heap_memory_max_mb.checked_mul(1024 * 1024).ok_or_else(|| {
            LibraryError::InvalidConfig {
                message: "heap_memory_max_mb is too large for this platform".to_string(),
            }
        })?,
        execution_timeout_secs: config.execution_timeout_secs,
        max_concurrent_executions: config.max_concurrent_executions as usize,
        session_id: config.session_id,
        session_fork_from: config.session_fork_from,
    })
}

fn storage_kind(kind: LibraryStorageKind) -> crate::cli::StoreKind {
    match kind {
        LibraryStorageKind::None => crate::cli::StoreKind::None,
        LibraryStorageKind::Directory => crate::cli::StoreKind::Dir,
        LibraryStorageKind::S3 => crate::cli::StoreKind::S3,
    }
}

fn validate_config(config: &LibraryConfig) -> Result<(), LibraryError> {
    if config.heap_memory_max_mb < crate::engine::MIN_HEAP_MEMORY_MB as u64 {
        return Err(LibraryError::InvalidConfig {
            message: format!(
                "heap_memory_max_mb must be at least {}",
                crate::engine::MIN_HEAP_MEMORY_MB
            ),
        });
    }
    if config.execution_timeout_secs == 0 {
        return Err(LibraryError::InvalidConfig {
            message: "execution_timeout_secs must be greater than zero".to_string(),
        });
    }
    if config.max_concurrent_executions == 0 {
        return Err(LibraryError::InvalidConfig {
            message: "max_concurrent_executions must be greater than zero".to_string(),
        });
    }
    if matches!(config.mode, LibraryMode::LocalStateful) && config.data_dir.is_none() {
        return Err(LibraryError::InvalidConfig {
            message: "data_dir is required in local_stateful mode".to_string(),
        });
    }
    Ok(())
}

fn build_runtime(
    config: &LibraryConfig,
) -> Result<(McpJsRuntime, Option<tempfile::TempDir>), LibraryError> {
    let heap_memory_max_mb =
        usize::try_from(config.heap_memory_max_mb).map_err(|_| LibraryError::InvalidConfig {
            message: "heap_memory_max_mb is too large for this platform".to_string(),
        })?;
    let ephemeral_data_dir =
        if matches!(config.mode, LibraryMode::Stateless) && config.data_dir.is_none() {
            Some(
                tempfile::tempdir().map_err(|error| LibraryError::Initialization {
                    message: format!("failed to create temporary data directory: {error}"),
                })?,
            )
        } else {
            None
        };
    let data_dir = config
        .data_dir
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            ephemeral_data_dir
                .as_ref()
                .expect("temporary directory created")
                .path()
                .to_path_buf()
        });

    let builder = McpJsRuntime::builder()
        .heap_memory_max_mb(heap_memory_max_mb)
        .execution_timeout_secs(config.execution_timeout_secs)
        .max_concurrent_executions(config.max_concurrent_executions as usize)
        .filesystem_enabled(config.filesystem_enabled);
    let builder = match config.mode {
        LibraryMode::Stateless => builder.stateless(data_dir),
        LibraryMode::LocalStateful => builder.local_stateful(data_dir),
    };
    let runtime = builder.build().map_err(init_message)?;
    Ok((runtime, ephemeral_data_dir))
}

fn library_mcp_headers_value(headers: LibraryMcpRequestHeaders) -> Value {
    Value::Object(
        headers
            .values
            .into_iter()
            .map(|(name, value)| (name, Value::String(value)))
            .collect(),
    )
}

fn parse_json_object(field: &str, json: &str) -> Result<Value, LibraryError> {
    let value: Value = serde_json::from_str(json).map_err(|error| LibraryError::InvalidJson {
        field: field.to_string(),
        message: error.to_string(),
    })?;
    if !value.is_object() {
        return Err(LibraryError::InvalidJson {
            field: field.to_string(),
            message: "expected a JSON object".to_string(),
        });
    }
    Ok(value)
}

fn init_message(message: String) -> LibraryError {
    LibraryError::Initialization { message }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn v8_test_guard() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

    #[test]
    fn typed_mcp_headers_convert_to_policy_json() {
        let headers = LibraryMcpRequestHeaders {
            values: HashMap::from([
                ("session-id".to_string(), "session-123".to_string()),
                ("tenant".to_string(), "acme".to_string()),
            ]),
        };

        assert_eq!(
            library_mcp_headers_value(headers),
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

    #[test]
    fn local_stateful_requires_data_dir() {
        let config = LibraryConfig {
            mode: LibraryMode::LocalStateful,
            data_dir: None,
            ..LibraryConfig::default()
        };
        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn runtime_config_builds_directory_storage_with_owned_executor() {
        let _guard = v8_test_guard();
        let data_dir = tempfile::tempdir().unwrap();
        let mut config = default_runtime_config(data_dir.path().to_string_lossy().into_owned());
        config.heap_store = LibraryStorageKind::Directory;
        config.heap_dir = Some(data_dir.path().join("heaps").to_string_lossy().into_owned());
        config.filesystem_store = LibraryStorageKind::Directory;
        config.filesystem_dir = Some(
            data_dir
                .path()
                .join("fs-blobs")
                .to_string_lossy()
                .into_owned(),
        );
        let library = create_library(config).unwrap();

        let capabilities = library.capabilities();
        assert!(capabilities.heap);
        assert!(capabilities.filesystem);
        assert!(capabilities.sessions);
        let tools = library.list_tools().unwrap();
        assert!(tools.iter().any(|tool| tool.name == "get_heap_tags"));
        assert!(tools.iter().any(|tool| tool.name == "fs_ls"));
    }

    #[test]
    fn runtime_config_requires_bucket_for_s3() {
        let data_dir = tempfile::tempdir().unwrap();
        let mut config = default_runtime_config(data_dir.path().to_string_lossy().into_owned());
        config.heap_store = LibraryStorageKind::S3;
        assert!(create_library(config).is_err());
    }

    #[test]
    fn upstream_mcp_config_deserializes_existing_json_shape() {
        let stdio: LibraryMcpServerConfig = serde_json::from_str(
            r#"{"name":"weather","transport":"stdio","command":"python","args":["server.py"],"env":{"TOKEN":"x"}}"#,
        )
        .unwrap();
        assert!(matches!(stdio.transport, LibraryMcpTransportKind::Stdio));
        assert_eq!(stdio.command.as_deref(), Some("python"));
        assert_eq!(stdio.args, ["server.py"]);
        assert_eq!(stdio.env.get("TOKEN").map(String::as_str), Some("x"));

        let sse: LibraryMcpServerConfig = serde_json::from_str(
            r#"{"name":"remote","transport":"sse","url":"http://127.0.0.1/sse"}"#,
        )
        .unwrap();
        assert!(matches!(sse.transport, LibraryMcpTransportKind::Sse));
        assert_eq!(sse.url.as_deref(), Some("http://127.0.0.1/sse"));
    }

    #[test]
    fn upstream_mcp_config_rejects_duplicate_names_before_connecting() {
        let _guard = v8_test_guard();
        let data_dir = tempfile::tempdir().unwrap();
        let server = LibraryMcpServerConfig {
            name: "duplicate".to_string(),
            transport: LibraryMcpTransportKind::Stdio,
            command: Some("true".to_string()),
            args: Vec::new(),
            env: HashMap::new(),
            url: None,
        };
        let upstreams = LibraryUpstreamMcpConfig {
            servers: vec![server.clone(), server],
            stubs: default_upstream_mcp_config().stubs,
        };
        let result = create_library_with_upstreams(
            default_runtime_config(data_dir.path().to_string_lossy().into_owned()),
            default_feature_config(),
            default_policy_config(),
            default_capability_config(),
            upstreams,
        );
        let error = match result {
            Ok(_) => panic!("duplicate upstream names should be rejected"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("duplicate MCP server name"));
    }

    #[test]
    fn configured_capabilities_allow_run_js_file() {
        let _guard = v8_test_guard();
        let data_dir = tempfile::tempdir().unwrap();
        let script = data_dir.path().join("script.js");
        std::fs::write(&script, "console.log(6 * 7)").unwrap();
        let capabilities = LibraryCapabilityConfig {
            run_js_file_access: LibraryRunJsFileAccess::AllowAll,
            ..default_capability_config()
        };
        let library = create_library_with_configuration(
            default_runtime_config(data_dir.path().to_string_lossy().into_owned()),
            default_feature_config(),
            default_policy_config(),
            capabilities,
        )
        .unwrap();

        let result = library
            .call_tool(
                "run_js".to_string(),
                serde_json::json!({ "file": script.to_string_lossy() }).to_string(),
                None,
                None,
            )
            .unwrap();
        let value: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(value["output"], "42");
    }

    #[test]
    fn configured_policies_reject_invalid_sources() {
        let _guard = v8_test_guard();
        let data_dir = tempfile::tempdir().unwrap();
        let policies = LibraryPolicyConfig {
            fetch: Some(LibraryOperationPolicies {
                mode: LibraryPolicyEvalMode::All,
                policies: vec![LibraryPolicySource {
                    url: "ftp://invalid".to_string(),
                    policy_path: None,
                    rule: None,
                }],
            }),
            ..default_policy_config()
        };
        let result = create_library_with_configuration(
            default_runtime_config(data_dir.path().to_string_lossy().into_owned()),
            default_feature_config(),
            policies,
            default_capability_config(),
        );
        let error = match result {
            Ok(_) => panic!("invalid policy source should be rejected"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("Unsupported policy URL scheme"));
    }

    #[test]
    fn configured_features_reject_wasm_with_heap_persistence() {
        let _guard = v8_test_guard();
        let data_dir = tempfile::tempdir().unwrap();
        let mut runtime_config =
            default_runtime_config(data_dir.path().to_string_lossy().into_owned());
        runtime_config.heap_store = LibraryStorageKind::Directory;
        runtime_config.heap_dir =
            Some(data_dir.path().join("heaps").to_string_lossy().into_owned());
        let mut features = default_feature_config();
        features.wasm_modules.push(LibraryWasmModuleConfig {
            name: "math".to_string(),
            bytes: b"\0asm".to_vec(),
            max_memory_bytes: None,
            description: None,
        });

        let error = match create_library_with_features(runtime_config, features) {
            Ok(_) => panic!("heap persistence with WASM should be rejected"),
            Err(error) => error,
        };
        assert!(
            error
                .to_string()
                .contains("incompatible with heap persistence")
        );
    }

    #[test]
    fn configured_features_apply_through_exported_factory() {
        let _guard = v8_test_guard();
        let data_dir = tempfile::tempdir().unwrap();
        let mut features = default_feature_config();
        features.instructions_override = Some("Custom instructions".to_string());
        features.run_js_description_override = Some("Custom run_js".to_string());
        features.wasm_modules.push(LibraryWasmModuleConfig {
            name: "math".to_string(),
            bytes: b"\0asm".to_vec(),
            max_memory_bytes: Some(1024 * 1024),
            description: Some("Math helpers".to_string()),
        });
        features.wasm_stubs.prefix = "ffi__".to_string();

        let library = create_library_with_features(
            default_runtime_config(data_dir.path().to_string_lossy().into_owned()),
            features,
        )
        .unwrap();
        assert_eq!(
            library.instructions_override().as_deref(),
            Some("Custom instructions")
        );
        assert_eq!(
            library.run_js_description_override().as_deref(),
            Some("Custom run_js")
        );
        assert!(
            library
                .list_tools()
                .unwrap()
                .iter()
                .any(|tool| tool.name == "ffi__wasm__math")
        );
    }

    #[test]
    fn configured_filesystem_uses_typed_label_api() {
        let _guard = v8_test_guard();
        let data_dir = tempfile::tempdir().unwrap();
        let library = McpJsLibrary::new(LibraryConfig {
            mode: LibraryMode::Stateless,
            data_dir: Some(data_dir.path().to_string_lossy().into_owned()),
            filesystem_enabled: true,
            ..LibraryConfig::default()
        })
        .unwrap();
        let runtime = library.tokio_runtime.as_ref().unwrap();
        let first = "0".repeat(64);
        let second = "1".repeat(64);

        assert!(library.capabilities().filesystem);
        runtime
            .block_on(library.fs_set_label(
                "main".to_string(),
                first.clone(),
                Some("create".to_string()),
            ))
            .unwrap();
        assert_eq!(
            runtime
                .block_on(library.fs_resolve_label("main".to_string()))
                .unwrap(),
            Some(first.clone())
        );
        assert_eq!(runtime.block_on(library.fs_list_labels()).unwrap().len(), 1);

        let pushed = runtime
            .block_on(library.fs_push(
                "main".to_string(),
                second.clone(),
                Some(first.clone()),
                false,
                Some("advance".to_string()),
            ))
            .unwrap();
        assert_eq!(pushed.status, "advanced");
        assert_eq!(pushed.ca_id, Some(second));
        assert_eq!(
            runtime
                .block_on(library.fs_label_log("main".to_string(), None))
                .unwrap()
                .len(),
            2
        );

        runtime
            .block_on(library.fs_reset(
                "main".to_string(),
                first.clone(),
                false,
                Some("rollback".to_string()),
            ))
            .unwrap();
        assert_eq!(
            runtime
                .block_on(library.fs_resolve_label("main".to_string()))
                .unwrap(),
            Some(first)
        );
    }

    #[test]
    fn local_stateful_sessions_and_heap_tags_use_typed_api() {
        let _guard = v8_test_guard();
        let data_dir = tempfile::tempdir().unwrap();
        let library = McpJsLibrary::new(LibraryConfig {
            mode: LibraryMode::LocalStateful,
            data_dir: Some(data_dir.path().to_string_lossy().into_owned()),
            ..LibraryConfig::default()
        })
        .unwrap();
        let runtime = library.tokio_runtime.as_ref().unwrap();

        assert!(
            runtime
                .block_on(library.list_sessions())
                .unwrap()
                .is_empty()
        );

        let tags = HashMap::from([
            ("environment".to_string(), "test".to_string()),
            ("owner".to_string(), "uniffi".to_string()),
        ]);
        runtime
            .block_on(library.set_heap_tags("heap-1".to_string(), tags.clone()))
            .unwrap();
        assert_eq!(
            runtime
                .block_on(library.get_heap_tags("heap-1".to_string()))
                .unwrap(),
            tags
        );

        let matches =
            runtime
                .block_on(library.query_heaps_by_tags(HashMap::from([(
                    "owner".to_string(),
                    "uniffi".to_string(),
                )])))
                .unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].heap, "heap-1");

        runtime
            .block_on(library.delete_heap_tags("heap-1".to_string(), None))
            .unwrap();
        assert!(
            runtime
                .block_on(library.get_heap_tags("heap-1".to_string()))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn stateless_run_js_executes_through_sync_and_async_library_apis() {
        let _guard = v8_test_guard();
        let library = McpJsLibrary::new(LibraryConfig::default()).unwrap();
        let result = library
            .call_tool(
                "run_js".to_string(),
                r#"{"code":"console.log(1 + 1)"}"#.to_string(),
                None,
                None,
            )
            .unwrap();
        let value: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(value["output"], "2");

        let runtime = library.tokio_runtime.as_ref().unwrap();
        let result = runtime
            .block_on(library.invoke_tool(LibraryToolCallRequest {
                name: "run_js".to_string(),
                arguments_json: r#"{"code":"console.log(2 + 2)"}"#.to_string(),
                session_id: None,
                mcp_headers: None,
            }))
            .unwrap();
        let value: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(value["output"], "4");
    }

    #[test]
    fn lifecycle_shutdown_is_idempotent_and_rejects_new_work() {
        let _guard = v8_test_guard();
        let library = McpJsLibrary::new(LibraryConfig::default()).unwrap();
        assert_eq!(library.lifecycle_state(), LibraryLifecycleState::Running);

        let runtime = library.tokio_runtime.as_ref().unwrap();
        let first = runtime.block_on(library.shutdown());
        assert!(!first.already_shutdown);
        assert_eq!(library.lifecycle_state(), LibraryLifecycleState::Shutdown);

        let second = runtime.block_on(library.shutdown());
        assert!(second.already_shutdown);
        let error = runtime
            .block_on(library.invoke_tool(LibraryToolCallRequest {
                name: "run_js".to_string(),
                arguments_json: r#"{"code":"console.log('late')"}"#.to_string(),
                session_id: None,
                mcp_headers: None,
            }))
            .unwrap_err();
        assert!(error.to_string().contains("library is Shutdown"));
    }

    #[test]
    fn local_stateful_tools_submit_poll_and_read_output() {
        let _guard = v8_test_guard();
        let data_dir = tempfile::tempdir().unwrap();
        let library = McpJsLibrary::new(LibraryConfig {
            mode: LibraryMode::LocalStateful,
            data_dir: Some(data_dir.path().to_string_lossy().into_owned()),
            ..LibraryConfig::default()
        })
        .unwrap();

        let runtime = library.tokio_runtime.as_ref().unwrap();
        let execution_id = runtime
            .block_on(library.submit_execution(LibraryExecutionRequest {
                code: "console.log(40 + 2)".to_string(),
                file: None,
                heap: None,
                fs: None,
                session: Some("ffi-test".to_string()),
                heap_memory_max_mb: None,
                execution_timeout_secs: None,
                tags: None,
                mcp_headers: None,
            }))
            .unwrap();

        let mut completed = false;
        for _ in 0..200 {
            let status = library.get_execution(execution_id.clone()).unwrap();
            if status.status == "completed" {
                completed = true;
                break;
            }
            if matches!(status.status.as_str(), "failed" | "timed_out" | "cancelled") {
                panic!("stateful execution failed: {}", status.status);
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        assert!(completed, "stateful execution did not complete");

        let output = library
            .get_execution_output(execution_id, None, None, None, None)
            .unwrap();
        assert_eq!(output.data, "42");
    }
}
