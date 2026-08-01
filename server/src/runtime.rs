//! Canonical runtime surface — the single uniffi-exported object shared by
//! embedded callers (FFI bindings) and every Rust server transport.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::cluster::ClusterNode;
use crate::engine::execution::{
    ConsoleOutputPage, ExecutionInfo, ExecutionRegistry, ExecutionSummary,
};
use crate::engine::fs_labels::LabelStore;
use crate::engine::fs_merge::Prefer;
use crate::engine::fs_store::FsStore;
use crate::engine::heap_storage::{AnyHeapStorage, FileHeapStorage};
use crate::engine::heap_tags::{HeapTagEntry, HeapTagStore};
use crate::engine::session_log::SessionLog;
use crate::engine::{
    Engine, FsLabelView, FsMergeResult, FsPushOutcome, FsRefLogView, MIN_HEAP_MEMORY_MB,
    RunJsRequest, initialize_v8,
};
use crate::mcp::{ToolCatalog, built_in_tool_catalog};

const DEFAULT_HEAP_MEMORY_MB: u64 = 64;
pub const DEFAULT_EXECUTION_TIMEOUT_SECS: u64 = crate::engine::DEFAULT_EXECUTION_TIMEOUT_SECS;
pub const DEFAULT_WASM_STUB_PREFIX: &str = crate::engine::wasm_stub::DEFAULT_WASM_STUB_PREFIX;
pub const DEFAULT_MCP_STUB_PREFIX: &str = crate::engine::mcp_client::DEFAULT_STUB_PREFIX;
const DEFAULT_MAX_CONCURRENT_EXECUTIONS: u32 = 4;

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

#[derive(Clone, Debug, Default, uniffi::Record)]
pub struct RuntimeHardeningConfig {
    pub freeze_ops: bool,
    pub neutralize_proxy_details: bool,
    pub neutralize_introspection: bool,
    pub remove_bootstrap: bool,
    pub remove_shared_memory: bool,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct RuntimeWasmModuleConfig {
    pub name: String,
    pub bytes: Vec<u8>,
    pub max_memory_bytes: Option<u64>,
    pub description: Option<String>,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct RuntimeWasmStubConfig {
    pub prefix: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct RuntimeFeatureConfig {
    pub wasm_default_max_bytes: u64,
    pub hardening: RuntimeHardeningConfig,
    pub wasm_modules: Vec<RuntimeWasmModuleConfig>,
    pub wasm_stubs: RuntimeWasmStubConfig,
    pub instructions_override: Option<String>,
    pub run_js_description_override: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, uniffi::Enum)]
#[serde(rename_all = "lowercase")]
pub enum RuntimePolicyEvalMode {
    #[default]
    All,
    Any,
}

#[derive(Clone, Debug, Deserialize, uniffi::Record)]
pub struct RuntimePolicySource {
    pub url: String,
    pub policy_path: Option<String>,
    pub rule: Option<String>,
}

#[derive(Clone, Debug, Deserialize, uniffi::Record)]
pub struct RuntimeOperationPolicies {
    #[serde(default)]
    pub mode: RuntimePolicyEvalMode,
    pub policies: Vec<RuntimePolicySource>,
}

#[derive(Clone, Debug, Default, Deserialize, uniffi::Record)]
pub struct RuntimePolicyConfig {
    pub fetch: Option<RuntimeOperationPolicies>,
    pub modules: Option<RuntimeOperationPolicies>,
    pub filesystem: Option<RuntimeOperationPolicies>,
    pub fs_snapshot: Option<RuntimeOperationPolicies>,
    pub mcp_tools: Option<RuntimeOperationPolicies>,
    pub subprocess: Option<RuntimeOperationPolicies>,
    pub run_js_file: Option<RuntimeOperationPolicies>,
}

#[derive(Clone, uniffi::Record)]
pub struct RuntimeFetchOAuthConfig {
    pub header_name: String,
    pub token_url: String,
    pub client_id: String,
    pub client_secret: String,
    pub scope: Option<String>,
    pub refresh_buffer_secs: u64,
}

impl std::fmt::Debug for RuntimeFetchOAuthConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RuntimeFetchOAuthConfig")
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
pub struct RuntimeFetchHeaderRule {
    pub host: String,
    pub methods: Vec<String>,
    pub static_headers: Option<HashMap<String, String>>,
    pub oauth: Option<RuntimeFetchOAuthConfig>,
}

impl RuntimeFetchHeaderRule {
    pub fn validate(&self) -> Result<(), RuntimeError> {
        crate::bootstrap::validate_fetch_header_rule(self)
    }

    pub fn normalized(self) -> Result<Self, RuntimeError> {
        crate::bootstrap::normalize_fetch_header_rule(self)
    }

    pub fn methods(&self) -> &[String] {
        &self.methods
    }

    pub fn static_headers(&self) -> Option<&HashMap<String, String>> {
        self.static_headers.as_ref()
    }

    pub fn dynamic_auth(&self) -> Option<&RuntimeFetchOAuthConfig> {
        self.oauth.as_ref()
    }
}

#[derive(Clone, Copy, Debug, Default, uniffi::Enum)]
pub enum RuntimeRunJsFileAccess {
    #[default]
    Disabled,
    AllowAll,
    Policy,
}

#[derive(Clone, Debug, Default, uniffi::Record)]
pub struct RuntimeCapabilityConfig {
    pub fetch_header_rules: Vec<RuntimeFetchHeaderRule>,
    pub filesystem_passthrough: bool,
    pub allow_external_modules: bool,
    pub run_js_file_access: RuntimeRunJsFileAccess,
}

#[derive(Clone, Copy, Debug, uniffi::Enum)]
pub enum RuntimeMcpTransportKind {
    Stdio,
    Sse,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct RuntimeMcpServerConfig {
    pub name: String,
    pub transport: RuntimeMcpTransportKind,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub url: Option<String>,
}

impl<'de> Deserialize<'de> for RuntimeMcpServerConfig {
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
                transport: RuntimeMcpTransportKind::Stdio,
                command: Some(command),
                args,
                env,
                url: None,
            },
            Transport::Sse { url } => Self {
                name: config.name,
                transport: RuntimeMcpTransportKind::Sse,
                command: None,
                args: Vec::new(),
                env: HashMap::new(),
                url: Some(url),
            },
        })
    }
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct RuntimeMcpStubConfig {
    pub prefix: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct RuntimeUpstreamMcpConfig {
    pub servers: Vec<RuntimeMcpServerConfig>,
    pub stubs: RuntimeMcpStubConfig,
}

#[derive(Clone, Copy, Debug, uniffi::Enum)]
pub enum RuntimeStorageKind {
    None,
    Directory,
    S3,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct RuntimeConfig {
    pub heap_store: RuntimeStorageKind,
    pub heap_dir: Option<String>,
    pub filesystem_store: RuntimeStorageKind,
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
pub struct RuntimeOptions {
    pub mode: RuntimeMode,
    pub data_dir: Option<String>,
    pub heap_memory_max_mb: u64,
    pub execution_timeout_secs: u64,
    pub max_concurrent_executions: u32,
    pub filesystem_enabled: bool,
}

impl Default for RuntimeOptions {
    fn default() -> Self {
        Self {
            mode: RuntimeMode::Stateless,
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
}

#[derive(uniffi::Object)]
pub struct McpJsRuntime {
    tokio_runtime: Option<tokio::runtime::Runtime>,
    engine: Engine,
    cluster_node: Option<Arc<ClusterNode>>,
    lifecycle: AtomicU8,
    shutdown_lock: tokio::sync::Mutex<()>,
    _ephemeral_data_dir: Option<tempfile::TempDir>,
}

#[uniffi::export]
pub fn default_runtime_options() -> RuntimeOptions {
    RuntimeOptions::default()
}

#[uniffi::export]
pub fn default_feature_config() -> RuntimeFeatureConfig {
    RuntimeFeatureConfig {
        wasm_default_max_bytes: crate::engine::DEFAULT_WASM_MAX_BYTES as u64,
        hardening: RuntimeHardeningConfig::default(),
        wasm_modules: Vec::new(),
        wasm_stubs: RuntimeWasmStubConfig {
            prefix: DEFAULT_WASM_STUB_PREFIX.to_string(),
            enabled: true,
        },
        instructions_override: None,
        run_js_description_override: None,
    }
}

#[uniffi::export]
pub fn default_policy_config() -> RuntimePolicyConfig {
    RuntimePolicyConfig::default()
}

#[uniffi::export]
pub fn default_fetch_oauth_refresh_buffer_secs() -> u64 {
    crate::engine::fetch::default_refresh_buffer_secs()
}

#[uniffi::export]
pub fn default_capability_config() -> RuntimeCapabilityConfig {
    RuntimeCapabilityConfig::default()
}

#[uniffi::export]
pub fn default_upstream_mcp_config() -> RuntimeUpstreamMcpConfig {
    RuntimeUpstreamMcpConfig {
        servers: Vec::new(),
        stubs: RuntimeMcpStubConfig {
            prefix: DEFAULT_MCP_STUB_PREFIX.to_string(),
            enabled: true,
        },
    }
}

#[uniffi::export]
pub fn default_runtime_config(data_dir: String) -> RuntimeConfig {
    RuntimeConfig {
        heap_store: RuntimeStorageKind::None,
        heap_dir: None,
        filesystem_store: RuntimeStorageKind::None,
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
pub fn create_runtime(config: RuntimeConfig) -> Result<Arc<McpJsRuntime>, RuntimeError> {
    create_runtime_with_features(config, default_feature_config())
}

#[uniffi::export]
pub fn create_runtime_with_features(
    config: RuntimeConfig,
    features: RuntimeFeatureConfig,
) -> Result<Arc<McpJsRuntime>, RuntimeError> {
    create_runtime_with_configuration(
        config,
        features,
        default_policy_config(),
        default_capability_config(),
    )
}

#[uniffi::export]
pub fn create_runtime_with_configuration(
    config: RuntimeConfig,
    features: RuntimeFeatureConfig,
    policies: RuntimePolicyConfig,
    capabilities: RuntimeCapabilityConfig,
) -> Result<Arc<McpJsRuntime>, RuntimeError> {
    create_runtime_with_upstreams(
        config,
        features,
        policies,
        capabilities,
        default_upstream_mcp_config(),
    )
}

#[uniffi::export]
pub fn create_runtime_with_upstreams(
    config: RuntimeConfig,
    features: RuntimeFeatureConfig,
    policies: RuntimePolicyConfig,
    capabilities: RuntimeCapabilityConfig,
    upstreams: RuntimeUpstreamMcpConfig,
) -> Result<Arc<McpJsRuntime>, RuntimeError> {
    validate_runtime_config(&config)?;
    initialize_v8();
    let worker_threads = usize::try_from(config.max_concurrent_executions).map_err(|_| {
        RuntimeError::InvalidConfig {
            message: "max_concurrent_executions is too large for this platform".to_string(),
        }
    })?;
    let tokio_runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(worker_threads)
        .build()
        .map_err(|error| RuntimeError::Initialization {
            message: error.to_string(),
        })?;
    let bootstrap_config = runtime_bootstrap_config(config)?;
    let bootstrap = tokio_runtime.block_on(async {
        let bootstrap = crate::bootstrap::build_storage_engine(bootstrap_config, None)
            .await
            .map_err(|error| RuntimeError::Initialization {
                message: error.to_string(),
            })?
            .with_feature_config(features)?
            .with_policy_config(policies, capabilities)?;
        bootstrap.with_upstream_mcp_config(upstreams).await
    })?;
    Ok(bootstrap.build_with_runtime(tokio_runtime))
}

#[uniffi::export]
impl McpJsRuntime {
    #[uniffi::constructor]
    pub fn new(config: RuntimeOptions) -> Result<Arc<Self>, RuntimeError> {
        validate_options(&config)?;
        initialize_v8();

        let tokio_runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(config.max_concurrent_executions as usize)
            .build()
            .map_err(|error| RuntimeError::Initialization {
                message: error.to_string(),
            })?;

        let (engine, ephemeral_data_dir) = build_engine_from_options(&config)?;
        Ok(Self::wrap(
            engine,
            Some(tokio_runtime),
            ephemeral_data_dir,
            None,
        ))
    }

    pub fn mode(&self) -> RuntimeMode {
        if self.engine.session_capable() {
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
        let (cancelled_executions, closed_mcp_connections) = self.engine.shutdown().await;
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
            heap: self.engine.heap_enabled(),
            filesystem: self.engine.fs_enabled(),
            sessions: self.engine.session_capable(),
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

        let mut execution = self.engine.run_js(request.code)
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
        self.engine
            .get_execution(&execution_id)
            .map_err(operation_message)
    }

    pub fn get_execution_output(
        &self,
        execution_id: String,
        line_offset: Option<u64>,
        line_limit: Option<u64>,
        byte_offset: Option<u64>,
        byte_limit: Option<u64>,
    ) -> Result<ConsoleOutputPage, RuntimeError> {
        self.engine
            .get_execution_output(
                &execution_id,
                line_offset,
                line_limit,
                byte_offset,
                byte_limit,
            )
            .map_err(operation_message)
    }

    pub fn cancel_execution(&self, execution_id: String) -> Result<(), RuntimeError> {
        self.engine
            .cancel_execution(&execution_id)
            .map_err(operation_message)
    }

    pub fn list_executions(&self) -> Result<Vec<ExecutionSummary>, RuntimeError> {
        self.engine.list_executions().map_err(operation_message)
    }

    pub async fn list_sessions(&self) -> Result<Vec<String>, RuntimeError> {
        self.engine
            .list_sessions()
            .await
            .map_err(operation_message)
    }

    pub async fn list_session_snapshots(
        &self,
        session: String,
        fields: Option<Vec<String>>,
    ) -> Result<Vec<String>, RuntimeError> {
        self.engine
            .list_session_snapshots(session, fields)
            .await
            .map_err(operation_message)?
            .into_iter()
            .map(|snapshot| {
                serde_json::to_string(&snapshot).map_err(|error| RuntimeError::Operation {
                    message: format!("failed to serialize session snapshot: {error}"),
                })
            })
            .collect()
    }

    pub async fn get_heap_tags(
        &self,
        heap: String,
    ) -> Result<HashMap<String, String>, RuntimeError> {
        self.engine
            .get_heap_tags(heap)
            .await
            .map_err(operation_message)
    }

    pub async fn set_heap_tags(
        &self,
        heap: String,
        tags: HashMap<String, String>,
    ) -> Result<(), RuntimeError> {
        self.engine
            .set_heap_tags(heap, tags)
            .await
            .map_err(operation_message)
    }

    pub async fn delete_heap_tags(
        &self,
        heap: String,
        keys: Option<Vec<String>>,
    ) -> Result<(), RuntimeError> {
        self.engine
            .delete_heap_tags(heap, keys)
            .await
            .map_err(operation_message)
    }

    pub async fn query_heaps_by_tags(
        &self,
        tags: HashMap<String, String>,
    ) -> Result<Vec<HeapTagEntry>, RuntimeError> {
        self.engine
            .query_heaps_by_tags(tags)
            .await
            .map_err(operation_message)
    }

    pub async fn fs_list_labels(&self) -> Result<Vec<FsLabelView>, RuntimeError> {
        self.engine.fs_list_labels().await.map_err(operation_message)
    }

    pub async fn fs_resolve_label(&self, name: String) -> Result<Option<String>, RuntimeError> {
        self.engine
            .fs_resolve_label(&name)
            .await
            .map_err(operation_message)
    }

    pub async fn fs_set_label(
        &self,
        name: String,
        ca_id: String,
        message: Option<String>,
    ) -> Result<(), RuntimeError> {
        self.engine
            .fs_set_label(&name, &ca_id, message)
            .await
            .map_err(operation_message)
    }

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
        self.engine
            .fs_label_log(&name, limit)
            .await
            .map_err(operation_message)
    }

    pub async fn fs_push(
        &self,
        label: String,
        ca_id: String,
        expected: Option<String>,
        force: bool,
        message: Option<String>,
    ) -> Result<FsPushOutcome, RuntimeError> {
        self.engine
            .fs_push(&label, &ca_id, expected, force, message)
            .await
            .map_err(operation_message)
    }

    pub async fn fs_reset(
        &self,
        label: String,
        ca_id: String,
        allow_unlogged: bool,
        message: Option<String>,
    ) -> Result<(), RuntimeError> {
        self.engine
            .fs_reset(&label, &ca_id, allow_unlogged, message)
            .await
            .map_err(operation_message)
    }

    pub async fn fs_merge(
        &self,
        ours: String,
        theirs: String,
        base: Option<String>,
        prefer: Prefer,
    ) -> Result<FsMergeResult, RuntimeError> {
        self.engine
            .fs_merge(&ours, &theirs, base, prefer)
            .await
            .map_err(operation_message)
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

    pub async fn invoke_tool(
        &self,
        request: ToolCallRequest,
    ) -> Result<String, RuntimeError> {
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

        serde_json::to_string(&result).map_err(|error| RuntimeError::ToolCall {
            message: format!("failed to serialize result: {error}"),
        })
    }
}

impl McpJsRuntime {
    /// Wrap a fully configured runtime for Rust transports without creating a
    /// second Tokio executor or crossing the FFI boundary.
    fn wrap(
        engine: Engine,
        tokio_runtime: Option<tokio::runtime::Runtime>,
        ephemeral_data_dir: Option<tempfile::TempDir>,
        cluster_node: Option<Arc<ClusterNode>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            tokio_runtime,
            engine,
            cluster_node,
            lifecycle: AtomicU8::new(RuntimeLifecycleState::Running as u8),
            shutdown_lock: tokio::sync::Mutex::new(()),
            _ephemeral_data_dir: ephemeral_data_dir,
        })
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

    fn ensure_running(&self) -> Result<(), RuntimeError> {
        match self.current_lifecycle_state() {
            RuntimeLifecycleState::Running => Ok(()),
            state => Err(RuntimeError::Operation {
                message: format!("runtime is {state:?}"),
            }),
        }
    }

    pub fn builder() -> McpJsRuntimeBuilder {
        McpJsRuntimeBuilder::default()
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

    pub fn heap_enabled(&self) -> bool {
        self.engine.heap_enabled()
    }

    pub fn fs_enabled(&self) -> bool {
        self.engine.fs_enabled()
    }

    pub fn session_capable(&self) -> bool {
        self.engine.session_capable()
    }

    pub fn tool_catalog(&self) -> ToolCatalog {
        built_in_tool_catalog(self.heap_enabled(), self.fs_enabled())
    }

    pub fn instructions_override(&self) -> Option<Arc<str>> {
        self.engine.instructions_override()
    }

    pub fn run_js_description_override(&self) -> Option<Arc<str>> {
        self.engine.run_js_description_override()
    }

    pub fn wasm_stub_tools(&self) -> Vec<rmcp::model::Tool> {
        self.engine.wasm_stub_tools()
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
        self.engine
            .mcp_client_manager()
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
    ) -> Value {
        if self.session_capable() {
            crate::mcp_dispatch::call_tool(&self.engine, session_id, mcp_headers, name, arguments)
                .await
        } else if name == "run_js" {
            crate::mcp_dispatch::run_js_blocking(&self.engine, mcp_headers, arguments).await
        } else {
            json!({ "error": format!("unknown stateless tool: {name}") })
        }
    }

    pub fn upstream_mcp_stub_call_response(
        &self,
        name: &str,
        arguments: Option<&serde_json::Map<String, Value>>,
    ) -> Option<rmcp::model::CallToolResult> {
        self.engine
            .mcp_client_manager()
            .and_then(|client| client.stub_call_response(name, arguments))
    }

    pub fn wasm_stub_call_response(
        &self,
        name: &str,
        arguments: Option<&serde_json::Map<String, Value>>,
    ) -> Option<rmcp::model::CallToolResult> {
        self.engine.wasm_stub_call_response(name, arguments)
    }

    pub fn run_js(&self, code: impl Into<String>) -> RunJsRequest<'_> {
        self.engine.run_js(code)
    }
}

fn operation_message(message: String) -> RuntimeError {
    RuntimeError::Operation { message }
}

fn validate_runtime_config(config: &RuntimeConfig) -> Result<(), RuntimeError> {
    if config.session_db_path.is_empty() {
        return Err(RuntimeError::InvalidConfig {
            message: "session_db_path must not be empty".to_string(),
        });
    }
    if config.heap_memory_max_mb < crate::engine::MIN_HEAP_MEMORY_MB as u64 {
        return Err(RuntimeError::InvalidConfig {
            message: format!(
                "heap_memory_max_mb must be at least {}",
                crate::engine::MIN_HEAP_MEMORY_MB
            ),
        });
    }
    if config.execution_timeout_secs == 0 || config.max_concurrent_executions == 0 {
        return Err(RuntimeError::InvalidConfig {
            message: "execution timeout and concurrency must be greater than zero".to_string(),
        });
    }
    let uses_s3 = matches!(config.heap_store, RuntimeStorageKind::S3)
        || matches!(config.filesystem_store, RuntimeStorageKind::S3);
    if uses_s3 && config.s3_bucket.is_none() {
        return Err(RuntimeError::InvalidConfig {
            message: "S3 storage requires s3_bucket".to_string(),
        });
    }
    Ok(())
}

fn runtime_bootstrap_config(
    config: RuntimeConfig,
) -> Result<crate::bootstrap::StorageBootstrapConfig, RuntimeError> {
    let heap_memory_max_mb =
        usize::try_from(config.heap_memory_max_mb).map_err(|_| RuntimeError::InvalidConfig {
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
            RuntimeError::InvalidConfig {
                message: "heap_memory_max_mb is too large for this platform".to_string(),
            }
        })?,
        execution_timeout_secs: config.execution_timeout_secs,
        max_concurrent_executions: config.max_concurrent_executions as usize,
        session_id: config.session_id,
        session_fork_from: config.session_fork_from,
    })
}

fn storage_kind(kind: RuntimeStorageKind) -> crate::cli::StoreKind {
    match kind {
        RuntimeStorageKind::None => crate::cli::StoreKind::None,
        RuntimeStorageKind::Directory => crate::cli::StoreKind::Dir,
        RuntimeStorageKind::S3 => crate::cli::StoreKind::S3,
    }
}

fn validate_options(config: &RuntimeOptions) -> Result<(), RuntimeError> {
    if config.heap_memory_max_mb < crate::engine::MIN_HEAP_MEMORY_MB as u64 {
        return Err(RuntimeError::InvalidConfig {
            message: format!(
                "heap_memory_max_mb must be at least {}",
                crate::engine::MIN_HEAP_MEMORY_MB
            ),
        });
    }
    if config.execution_timeout_secs == 0 {
        return Err(RuntimeError::InvalidConfig {
            message: "execution_timeout_secs must be greater than zero".to_string(),
        });
    }
    if config.max_concurrent_executions == 0 {
        return Err(RuntimeError::InvalidConfig {
            message: "max_concurrent_executions must be greater than zero".to_string(),
        });
    }
    if matches!(config.mode, RuntimeMode::LocalStateful) && config.data_dir.is_none() {
        return Err(RuntimeError::InvalidConfig {
            message: "data_dir is required in local_stateful mode".to_string(),
        });
    }
    Ok(())
}

fn build_engine_from_options(
    config: &RuntimeOptions,
) -> Result<(Engine, Option<tempfile::TempDir>), RuntimeError> {
    let heap_memory_max_mb =
        usize::try_from(config.heap_memory_max_mb).map_err(|_| RuntimeError::InvalidConfig {
            message: "heap_memory_max_mb is too large for this platform".to_string(),
        })?;
    let ephemeral_data_dir =
        if matches!(config.mode, RuntimeMode::Stateless) && config.data_dir.is_none() {
            Some(
                tempfile::tempdir().map_err(|error| RuntimeError::Initialization {
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
        RuntimeMode::Stateless => builder.stateless(data_dir),
        RuntimeMode::LocalStateful => builder.local_stateful(data_dir),
    };
    let engine = builder.build_engine().map_err(init_message)?;
    Ok((engine, ephemeral_data_dir))
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

fn init_message(message: String) -> RuntimeError {
    RuntimeError::Initialization { message }
}

#[derive(Clone, Debug)]
enum RuntimeStorage {
    Stateless { data_dir: PathBuf },
    LocalStateful { data_dir: PathBuf },
}

#[derive(Clone, Debug)]
pub struct McpJsRuntimeBuilder {
    storage: Option<RuntimeStorage>,
    heap_memory_max_mb: usize,
    execution_timeout_secs: u64,
    max_concurrent_executions: usize,
    filesystem_enabled: bool,
}

impl Default for McpJsRuntimeBuilder {
    fn default() -> Self {
        Self {
            storage: None,
            heap_memory_max_mb: DEFAULT_HEAP_MEMORY_MB as usize,
            execution_timeout_secs: crate::engine::DEFAULT_EXECUTION_TIMEOUT_SECS,
            max_concurrent_executions: DEFAULT_MAX_CONCURRENT_EXECUTIONS as usize,
            filesystem_enabled: false,
        }
    }
}

impl McpJsRuntimeBuilder {
    pub fn stateless(mut self, data_dir: impl Into<PathBuf>) -> Self {
        self.storage = Some(RuntimeStorage::Stateless {
            data_dir: data_dir.into(),
        });
        self
    }

    pub fn local_stateful(mut self, data_dir: impl Into<PathBuf>) -> Self {
        self.storage = Some(RuntimeStorage::LocalStateful {
            data_dir: data_dir.into(),
        });
        self
    }

    pub fn heap_memory_max_mb(mut self, heap_memory_max_mb: usize) -> Self {
        self.heap_memory_max_mb = heap_memory_max_mb;
        self
    }

    pub fn execution_timeout_secs(mut self, execution_timeout_secs: u64) -> Self {
        self.execution_timeout_secs = execution_timeout_secs;
        self
    }

    pub fn max_concurrent_executions(mut self, max_concurrent_executions: usize) -> Self {
        self.max_concurrent_executions = max_concurrent_executions;
        self
    }

    pub fn filesystem_enabled(mut self, filesystem_enabled: bool) -> Self {
        self.filesystem_enabled = filesystem_enabled;
        self
    }

    pub fn build(self) -> Result<std::sync::Arc<McpJsRuntime>, String> {
        self.build_engine().map(McpJsRuntime::from_engine)
    }

    pub fn build_engine(self) -> Result<Engine, String> {
        self.validate()?;
        let heap_memory_max_bytes = self
            .heap_memory_max_mb
            .checked_mul(1024 * 1024)
            .ok_or_else(|| "heap_memory_max_mb is too large for this platform".to_string())?;
        let storage = self
            .storage
            .ok_or_else(|| "runtime storage mode is required".to_string())?;

        match storage {
            RuntimeStorage::Stateless { data_dir } => {
                create_data_dir(&data_dir)?;
                let registry = execution_registry(&data_dir)?;
                let engine = Engine::new_stateless(
                    heap_memory_max_bytes,
                    self.execution_timeout_secs,
                    self.max_concurrent_executions,
                )
                .with_execution_registry(Arc::new(registry));
                configure_filesystem(engine, &data_dir, self.filesystem_enabled, true)
            }
            RuntimeStorage::LocalStateful { data_dir } => {
                create_data_dir(&data_dir)?;
                let session_log = SessionLog::new(path_string(&data_dir.join("sessions"))?)?;
                let heap_tags = HeapTagStore::new(path_string(&data_dir.join("heap-tags"))?)?;
                let registry = execution_registry(&data_dir)?;
                let engine = Engine::new_stateful(
                    AnyHeapStorage::File(FileHeapStorage::new(data_dir.join("heaps"))),
                    Some(session_log),
                    Some(heap_tags),
                    heap_memory_max_bytes,
                    self.execution_timeout_secs,
                    self.max_concurrent_executions,
                )
                .with_execution_registry(Arc::new(registry));
                configure_filesystem(engine, &data_dir, self.filesystem_enabled, false)
            }
        }
    }

    fn validate(&self) -> Result<(), String> {
        if self.heap_memory_max_mb < MIN_HEAP_MEMORY_MB {
            return Err(format!(
                "heap_memory_max_mb must be at least {MIN_HEAP_MEMORY_MB}"
            ));
        }
        if self.execution_timeout_secs == 0 {
            return Err("execution_timeout_secs must be greater than zero".to_string());
        }
        if self.max_concurrent_executions == 0 {
            return Err("max_concurrent_executions must be greater than zero".to_string());
        }
        Ok(())
    }
}

fn configure_filesystem(
    mut engine: Engine,
    data_dir: &Path,
    filesystem_enabled: bool,
    needs_session_log: bool,
) -> Result<Engine, String> {
    if !filesystem_enabled {
        return Ok(engine);
    }
    if needs_session_log {
        engine =
            engine.with_session_log(SessionLog::new(path_string(&data_dir.join("sessions"))?)?);
    }
    let backend = Arc::new(FileHeapStorage::new(data_dir.join("fs-blobs")));
    let store = Arc::new(FsStore::new(backend));
    let labels = Arc::new(LabelStore::new(path_string(&data_dir.join("fs-labels"))?)?);
    Ok(engine.with_fs_snapshots(store, labels))
}

fn create_data_dir(data_dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(data_dir)
        .map_err(|error| format!("failed to create '{}': {error}", data_dir.display()))
}

fn execution_registry(data_dir: &Path) -> Result<ExecutionRegistry, String> {
    ExecutionRegistry::new(path_string(&data_dir.join("executions"))?)
}

fn path_string(path: &Path) -> Result<&str, String> {
    path.to_str()
        .ok_or_else(|| format!("path is not valid UTF-8: {}", path.display()))
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

    #[test]
    fn local_stateful_requires_data_dir() {
        let config = RuntimeOptions {
            mode: RuntimeMode::LocalStateful,
            data_dir: None,
            ..RuntimeOptions::default()
        };
        assert!(validate_options(&config).is_err());
    }

    #[test]
    fn runtime_config_builds_directory_storage_with_owned_executor() {
        let _guard = v8_test_guard();
        let data_dir = tempfile::tempdir().unwrap();
        let mut config = default_runtime_config(data_dir.path().to_string_lossy().into_owned());
        config.heap_store = RuntimeStorageKind::Directory;
        config.heap_dir = Some(data_dir.path().join("heaps").to_string_lossy().into_owned());
        config.filesystem_store = RuntimeStorageKind::Directory;
        config.filesystem_dir = Some(
            data_dir
                .path()
                .join("fs-blobs")
                .to_string_lossy()
                .into_owned(),
        );
        let library = create_runtime(config).unwrap();

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
        config.heap_store = RuntimeStorageKind::S3;
        assert!(create_runtime(config).is_err());
    }

    #[test]
    fn upstream_mcp_config_deserializes_existing_json_shape() {
        let stdio: RuntimeMcpServerConfig = serde_json::from_str(
            r#"{"name":"weather","transport":"stdio","command":"python","args":["server.py"],"env":{"TOKEN":"x"}}"#,
        )
        .unwrap();
        assert!(matches!(stdio.transport, RuntimeMcpTransportKind::Stdio));
        assert_eq!(stdio.command.as_deref(), Some("python"));
        assert_eq!(stdio.args, ["server.py"]);
        assert_eq!(stdio.env.get("TOKEN").map(String::as_str), Some("x"));

        let sse: RuntimeMcpServerConfig = serde_json::from_str(
            r#"{"name":"remote","transport":"sse","url":"http://127.0.0.1/sse"}"#,
        )
        .unwrap();
        assert!(matches!(sse.transport, RuntimeMcpTransportKind::Sse));
        assert_eq!(sse.url.as_deref(), Some("http://127.0.0.1/sse"));
    }

    #[test]
    fn upstream_mcp_config_rejects_duplicate_names_before_connecting() {
        let _guard = v8_test_guard();
        let data_dir = tempfile::tempdir().unwrap();
        let server = RuntimeMcpServerConfig {
            name: "duplicate".to_string(),
            transport: RuntimeMcpTransportKind::Stdio,
            command: Some("true".to_string()),
            args: Vec::new(),
            env: HashMap::new(),
            url: None,
        };
        let upstreams = RuntimeUpstreamMcpConfig {
            servers: vec![server.clone(), server],
            stubs: default_upstream_mcp_config().stubs,
        };
        let result = create_runtime_with_upstreams(
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
        let capabilities = RuntimeCapabilityConfig {
            run_js_file_access: RuntimeRunJsFileAccess::AllowAll,
            ..default_capability_config()
        };
        let library = create_runtime_with_configuration(
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
        let policies = RuntimePolicyConfig {
            fetch: Some(RuntimeOperationPolicies {
                mode: RuntimePolicyEvalMode::All,
                policies: vec![RuntimePolicySource {
                    url: "ftp://invalid".to_string(),
                    policy_path: None,
                    rule: None,
                }],
            }),
            ..default_policy_config()
        };
        let result = create_runtime_with_configuration(
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
        runtime_config.heap_store = RuntimeStorageKind::Directory;
        runtime_config.heap_dir =
            Some(data_dir.path().join("heaps").to_string_lossy().into_owned());
        let mut features = default_feature_config();
        features.wasm_modules.push(RuntimeWasmModuleConfig {
            name: "math".to_string(),
            bytes: b"\0asm".to_vec(),
            max_memory_bytes: None,
            description: None,
        });

        let error = match create_runtime_with_features(runtime_config, features) {
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
        features.wasm_modules.push(RuntimeWasmModuleConfig {
            name: "math".to_string(),
            bytes: b"\0asm".to_vec(),
            max_memory_bytes: Some(1024 * 1024),
            description: Some("Math helpers".to_string()),
        });
        features.wasm_stubs.prefix = "ffi__".to_string();

        let library = create_runtime_with_features(
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
        let library = McpJsRuntime::new(RuntimeOptions {
            mode: RuntimeMode::Stateless,
            data_dir: Some(data_dir.path().to_string_lossy().into_owned()),
            filesystem_enabled: true,
            ..RuntimeOptions::default()
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
        match pushed {
            FsPushOutcome::Advanced { label, ca_id } => {
                assert_eq!(label, "main");
                assert_eq!(ca_id, second);
            }
            other => panic!("expected an advanced push, got {other:?}"),
        }
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
        let library = McpJsRuntime::new(RuntimeOptions {
            mode: RuntimeMode::LocalStateful,
            data_dir: Some(data_dir.path().to_string_lossy().into_owned()),
            ..RuntimeOptions::default()
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
        let library = McpJsRuntime::new(RuntimeOptions::default()).unwrap();
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
            .block_on(library.invoke_tool(ToolCallRequest {
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
        let library = McpJsRuntime::new(RuntimeOptions::default()).unwrap();
        assert_eq!(library.lifecycle_state(), RuntimeLifecycleState::Running);

        let runtime = library.tokio_runtime.as_ref().unwrap();
        let first = runtime.block_on(library.shutdown());
        assert!(!first.already_shutdown);
        assert_eq!(library.lifecycle_state(), RuntimeLifecycleState::Shutdown);

        let second = runtime.block_on(library.shutdown());
        assert!(second.already_shutdown);
        let error = runtime
            .block_on(library.invoke_tool(ToolCallRequest {
                name: "run_js".to_string(),
                arguments_json: r#"{"code":"console.log('late')"}"#.to_string(),
                session_id: None,
                mcp_headers: None,
            }))
            .unwrap_err();
        assert!(error.to_string().contains("runtime is Shutdown"));
    }

    #[test]
    fn local_stateful_tools_submit_poll_and_read_output() {
        let _guard = v8_test_guard();
        let data_dir = tempfile::tempdir().unwrap();
        let library = McpJsRuntime::new(RuntimeOptions {
            mode: RuntimeMode::LocalStateful,
            data_dir: Some(data_dir.path().to_string_lossy().into_owned()),
            ..RuntimeOptions::default()
        })
        .unwrap();

        let runtime = library.tokio_runtime.as_ref().unwrap();
        let execution_id = runtime
            .block_on(library.submit_execution(ExecutionRequest {
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

#[cfg(test)]
mod builder_tests {
    use super::*;

    #[test]
    fn builder_requires_storage_mode() {
        assert!(McpJsRuntime::builder().build().is_err());
    }

    #[test]
    fn stateless_builder_configures_execution_registry() {
        let data_dir = tempfile::tempdir().unwrap();
        let runtime = McpJsRuntime::builder()
            .stateless(data_dir.path())
            .build()
            .unwrap();

        assert!(!runtime.session_capable());
        assert!(runtime.list_executions().is_ok());
    }

    #[test]
    fn stateless_builder_can_enable_filesystem_snapshots() {
        let data_dir = tempfile::tempdir().unwrap();
        let runtime = McpJsRuntime::builder()
            .stateless(data_dir.path())
            .filesystem_enabled(true)
            .build()
            .unwrap();

        assert!(!runtime.heap_enabled());
        assert!(runtime.fs_enabled());
        assert!(runtime.session_capable());
    }

    #[test]
    fn local_stateful_builder_enables_heap_tools() {
        let data_dir = tempfile::tempdir().unwrap();
        let runtime = McpJsRuntime::builder()
            .local_stateful(data_dir.path())
            .build()
            .unwrap();

        assert!(runtime.heap_enabled());
        assert!(
            runtime
                .tool_catalog()
                .tools
                .iter()
                .any(|tool| tool.name == "get_heap_tags")
        );
    }
}
