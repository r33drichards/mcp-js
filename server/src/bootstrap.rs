//! Runtime storage bootstrap shared by the CLI and embedded library hosts.

use std::sync::Arc;

use anyhow::{Result, anyhow, bail};

use crate::cli::StoreKind;
use crate::cluster::ClusterNode;
use crate::engine::execution::ExecutionRegistry;
use crate::engine::fs_labels::LabelStore;
use crate::engine::fs_store::FsStore;
use crate::engine::heap_storage::{
    AnyHeapStorage, FileHeapStorage, HeapStorage, S3HeapStorage, WriteThroughCacheHeapStorage,
};
use crate::engine::heap_tags::HeapTagStore;
use crate::engine::session_log::{ForkOutcome, SessionLog};
use std::collections::{HashMap, HashSet};

use serde::Deserialize;

use crate::engine::fetch::{HeaderInjection, HeaderRule, OAuthClientCredentialsConfig};
use crate::engine::mcp_client::{McpServerConfig, McpServerTransport, StubConfig};
use crate::engine::opa::OperationPolicies;
use crate::engine::wasm_stub::WasmStubConfig;
use crate::engine::{
    DEFAULT_WASM_MAX_BYTES, Engine, HardeningConfig, RuntimeError, WasmModule, initialize_v8,
};

/// Embedded-feature configuration (hardening, WASM modules, prompt overrides),
/// expressed directly in the engine's native types.
pub struct FeatureBootstrapConfig {
    pub wasm_default_max_bytes: usize,
    pub hardening: HardeningConfig,
    pub wasm_modules: Vec<WasmModule>,
    pub wasm_stubs: WasmStubConfig,
    pub instructions_override: Option<String>,
    pub run_js_description_override: Option<String>,
}

impl Default for FeatureBootstrapConfig {
    fn default() -> Self {
        Self {
            wasm_default_max_bytes: DEFAULT_WASM_MAX_BYTES,
            hardening: HardeningConfig::default(),
            wasm_modules: Vec::new(),
            wasm_stubs: WasmStubConfig::default(),
            instructions_override: None,
            run_js_description_override: None,
        }
    }
}

/// Per-operation OPA policy chains, in the engine's native policy types.
/// Deserializes the same JSON shape `--policies-json` always used.
#[derive(Default, Deserialize)]
pub struct PolicyBootstrapConfig {
    pub fetch: Option<OperationPolicies>,
    pub modules: Option<OperationPolicies>,
    pub filesystem: Option<OperationPolicies>,
    pub fs_snapshot: Option<OperationPolicies>,
    pub mcp_tools: Option<OperationPolicies>,
    pub subprocess: Option<OperationPolicies>,
    pub run_js_file: Option<OperationPolicies>,
}

/// How `run_js` may read code from server-side file paths.
#[derive(Clone, Copy, Debug, Default)]
pub enum RunJsFileAccess {
    #[default]
    Disabled,
    AllowAll,
    Policy,
}

/// Sandbox capabilities granted to executions.
#[derive(Default)]
pub struct CapabilityBootstrapConfig {
    pub fetch_header_rules: Vec<HeaderRule>,
    pub filesystem_passthrough: bool,
    pub allow_external_modules: bool,
    pub run_js_file_access: RunJsFileAccess,
}

pub struct RuntimeBootstrap {
    engine: Engine,
    cluster_node: Option<Arc<ClusterNode>>,
}

impl RuntimeBootstrap {
    pub fn with_feature_config(
        mut self,
        config: FeatureBootstrapConfig,
    ) -> Result<Self, RuntimeError> {
        if self.engine.heap_enabled() && !config.wasm_modules.is_empty() {
            return Err(RuntimeError::InvalidConfig {
                message: "WASM modules are incompatible with heap persistence".to_string(),
            });
        }
        if config.wasm_stubs.prefix.is_empty() {
            return Err(RuntimeError::InvalidConfig {
                message: "WASM stub prefix cannot be empty".to_string(),
            });
        }

        let mut names = HashSet::new();
        for module in &config.wasm_modules {
            validate_wasm_module_name(&module.name)?;
            if !names.insert(module.name.clone()) {
                return Err(RuntimeError::InvalidConfig {
                    message: format!("duplicate WASM module name: '{}'", module.name),
                });
            }
        }

        self.engine = self
            .engine
            .with_wasm_default_max_bytes(config.wasm_default_max_bytes)
            .with_hardening(config.hardening);
        if !config.wasm_modules.is_empty() {
            self.engine = self
                .engine
                .with_wasm_modules(config.wasm_modules)
                .with_wasm_stub_config(config.wasm_stubs);
        }
        if let Some(text) = config.instructions_override {
            self.engine = self.engine.with_instructions_override(text);
        }
        if let Some(text) = config.run_js_description_override {
            self.engine = self.engine.with_run_js_description_override(text);
        }
        Ok(self)
    }

    pub fn with_policy_config(
        mut self,
        policies: PolicyBootstrapConfig,
        capabilities: CapabilityBootstrapConfig,
    ) -> Result<Self, RuntimeError> {
        let fetch_policy = build_policy_chain(
            policies.fetch,
            "mcp/fetch",
            "data.mcp.fetch.allow",
            "fetch",
        )?;
        let modules_policy = build_policy_chain(
            policies.modules,
            "mcp/modules",
            "data.mcp.modules.allow",
            "modules",
        )?;
        let filesystem_policy = build_policy_chain(
            policies.filesystem,
            "mcp/filesystem",
            "data.mcp.filesystem.allow",
            "filesystem",
        )?;
        let fs_snapshot_policy = build_policy_chain(
            policies.fs_snapshot,
            "mcp/fs_snapshot",
            "data.mcp.fs_snapshot.allow",
            "fs_snapshot",
        )?;
        let mcp_tools_policy = build_policy_chain(
            policies.mcp_tools,
            "mcp/tools",
            "data.mcp.tools.allow",
            "mcp_tools",
        )?;
        let subprocess_policy = build_policy_chain(
            policies.subprocess,
            "mcp/subprocess",
            "data.mcp.subprocess.allow",
            "subprocess",
        )?;
        let run_js_file_policy = build_policy_chain(
            policies.run_js_file,
            "mcp/run_js_file",
            "data.mcp.run_js_file.allow",
            "run_js_file",
        )?;
        let header_rules = capabilities.fetch_header_rules;

        if let Some(chain) = fetch_policy {
            self.engine = self.engine.with_fetch_config(
                crate::engine::fetch::FetchConfig::new_with_chain(chain)
                    .with_header_rules(header_rules),
            );
        }
        if let Some(chain) = filesystem_policy {
            self.engine = self.engine.with_fs_config(
                crate::engine::fs::FsConfig::new(chain)
                    .with_passthrough(capabilities.filesystem_passthrough),
            );
        } else if self.engine.fs_enabled() {
            self.engine = self.engine.with_fs_config(
                crate::engine::fs::FsConfig::new(Arc::new(crate::engine::opa::PolicyChain::new(
                    Vec::new(),
                    crate::engine::opa::EvalMode::All,
                )))
                .with_passthrough(capabilities.filesystem_passthrough),
            );
        }
        if let Some(chain) = fs_snapshot_policy {
            self.engine = self.engine.with_fs_snapshot_policy(chain);
        }
        self.engine = self.engine.with_module_loader_config(
            crate::engine::module_loader::ModuleLoaderConfig {
                allow_external: capabilities.allow_external_modules,
                policy_chain: modules_policy,
            },
        );
        if let Some(chain) = subprocess_policy {
            self.engine = self
                .engine
                .with_subprocess_config(crate::engine::subprocess::SubprocessConfig::new(chain));
        }
        match capabilities.run_js_file_access {
            RunJsFileAccess::AllowAll => {
                self.engine = self
                    .engine
                    .with_run_js_file_policy(crate::engine::run_js_file::RunJsFilePolicy::AllowAll);
            }
            RunJsFileAccess::Policy => {
                let chain = run_js_file_policy.ok_or_else(|| RuntimeError::InvalidConfig {
                    message: "run_js_file_access=Policy requires a run_js_file policy".to_string(),
                })?;
                self.engine = self.engine.with_run_js_file_policy(
                    crate::engine::run_js_file::RunJsFilePolicy::Policy(chain),
                );
            }
            RunJsFileAccess::Disabled => {
                if let Some(chain) = run_js_file_policy {
                    self.engine = self.engine.with_run_js_file_policy(
                        crate::engine::run_js_file::RunJsFilePolicy::Policy(chain),
                    );
                }
            }
        }
        if let Some(chain) = mcp_tools_policy {
            self.engine = self.engine.with_mcp_tools_policy_chain(chain);
        }
        Ok(self)
    }

    pub async fn with_upstream_mcp_config(
        mut self,
        servers: Vec<McpServerConfig>,
        stubs: StubConfig,
    ) -> Result<Self, RuntimeError> {
        if servers.is_empty() {
            return Ok(self);
        }
        if stubs.prefix.is_empty() {
            return Err(RuntimeError::InvalidConfig {
                message: "upstream MCP stub prefix cannot be empty".to_string(),
            });
        }

        let mut names = HashSet::new();
        for server in &servers {
            if !names.insert(server.name.clone()) {
                return Err(RuntimeError::InvalidConfig {
                    message: format!("duplicate MCP server name: '{}'", server.name),
                });
            }
            match &server.transport {
                McpServerTransport::Stdio { command, .. } if command.is_empty() => {
                    return Err(RuntimeError::InvalidConfig {
                        message: format!("stdio MCP server '{}' requires a command", server.name),
                    });
                }
                McpServerTransport::Sse { url } if url.is_empty() => {
                    return Err(RuntimeError::InvalidConfig {
                        message: format!("SSE MCP server '{}' requires a URL", server.name),
                    });
                }
                _ => {}
            }
        }

        let manager = crate::engine::mcp_client::McpClientManager::connect(servers)
            .await
            .map_err(|message| RuntimeError::Initialization {
                message: format!("MCP server connection failed: {message}"),
            })?
            .with_stub_config(stubs);
        self.engine = self.engine.with_mcp_client_manager(manager);
        Ok(self)
    }

    pub fn build(self) -> Arc<Engine> {
        Engine::from_engine_with_cluster(self.engine, self.cluster_node)
    }

    pub(crate) fn build_with_runtime(
        self,
        tokio_runtime: tokio::runtime::Runtime,
    ) -> Arc<Engine> {
        Engine::from_engine_with_tokio_runtime(self.engine, tokio_runtime, self.cluster_node)
    }
}

fn build_policy_chain(
    policies: Option<OperationPolicies>,
    default_remote_path: &str,
    default_local_rule: &str,
    operation: &str,
) -> Result<Option<Arc<crate::engine::opa::PolicyChain>>, RuntimeError> {
    let Some(policies) = policies else {
        return Ok(None);
    };
    crate::engine::opa::build_policy_chain(&policies, default_remote_path, default_local_rule)
        .map(Arc::new)
        .map(Some)
        .map_err(|message| RuntimeError::InvalidConfig {
            message: format!("failed to build {operation} policy chain: {message}"),
        })
}

/// Build a validated fetch header rule from its parts. Exactly one of
/// `static_headers` or `oauth` must be provided.
pub fn fetch_header_rule(
    host: String,
    methods: Vec<String>,
    static_headers: Option<HashMap<String, String>>,
    oauth: Option<OAuthClientCredentialsConfig>,
) -> Result<HeaderRule, RuntimeError> {
    let result = match (static_headers, oauth) {
        (Some(_), Some(_)) => {
            return Err(RuntimeError::InvalidConfig {
                message: format!(
                    "fetch header rule for host '{host}' cannot define both static_headers and oauth"
                ),
            });
        }
        (None, None) => {
            return Err(RuntimeError::InvalidConfig {
                message: format!(
                    "fetch header rule for host '{host}' must define static_headers or oauth"
                ),
            });
        }
        (Some(headers), None) => {
            HeaderRule::new(host, methods, HeaderInjection::Static { headers })
        }
        (None, Some(oauth)) => HeaderRule::oauth_client_credentials(host, methods, oauth),
    };
    result.map_err(|error| RuntimeError::InvalidConfig {
        message: format!("invalid fetch header rule: {error}"),
    })
}

fn validate_wasm_module_name(name: &str) -> Result<(), RuntimeError> {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return Err(RuntimeError::InvalidConfig {
            message: "WASM module name cannot be empty".to_string(),
        });
    };
    if !first.is_ascii_alphabetic() && first != '_' && first != '$' {
        return Err(RuntimeError::InvalidConfig {
            message: format!(
                "WASM module name '{}' must start with a letter, underscore, or dollar sign",
                name
            ),
        });
    }
    if let Some(invalid) = chars.find(|character| {
        !character.is_ascii_alphanumeric() && *character != '_' && *character != '$'
    }) {
        return Err(RuntimeError::InvalidConfig {
            message: format!(
                "WASM module name '{}' contains invalid character '{}'",
                name, invalid
            ),
        });
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub struct StorageBootstrapConfig {
    pub heap_store: StoreKind,
    pub heap_dir: Option<String>,
    pub fs_store: StoreKind,
    pub fs_dir: Option<String>,
    pub fs_labels_db: Option<String>,
    pub s3_bucket: Option<String>,
    pub cache_dir: Option<String>,
    pub session_db_path: String,
    pub http_port: Option<u16>,
    pub execution_db_path: Option<String>,
    pub heap_memory_max_bytes: usize,
    pub execution_timeout_secs: u64,
    pub max_concurrent_executions: usize,
    pub session_id: Option<String>,
    pub session_fork_from: Option<String>,
}

impl StorageBootstrapConfig {
    pub fn heap_enabled(&self) -> bool {
        self.heap_store != StoreKind::None
    }

    pub fn fs_enabled(&self) -> bool {
        self.fs_store != StoreKind::None
    }
}

pub async fn build_storage_engine(
    config: StorageBootstrapConfig,
    cluster_node: Option<Arc<ClusterNode>>,
) -> Result<RuntimeBootstrap> {
    initialize_v8();
    let heap_enabled = config.heap_enabled();

    let engine = if heap_enabled {
        let heap_storage = build_heap_storage(&config).await?;
        tracing::info!("Heap persistence: ENABLED");
        Engine::new_stateful(
            heap_storage,
            None,
            None,
            config.heap_memory_max_bytes,
            config.execution_timeout_secs,
            config.max_concurrent_executions,
        )
    } else {
        tracing::info!("Heap persistence: disabled");
        Engine::new_stateless(
            config.heap_memory_max_bytes,
            config.execution_timeout_secs,
            config.max_concurrent_executions,
        )
    };

    let engine = attach_session_log(engine, &config, cluster_node.as_ref()).await?;
    let engine = attach_heap_tags(engine, &config, cluster_node.as_ref());
    let engine = attach_filesystem(engine, &config, cluster_node.as_ref()).await?;
    Ok(RuntimeBootstrap {
        engine: attach_execution_registry(engine, &config),
        cluster_node,
    })
}

async fn build_heap_storage(config: &StorageBootstrapConfig) -> Result<AnyHeapStorage> {
    match config.heap_store {
        StoreKind::S3 => {
            let bucket = config
                .s3_bucket
                .clone()
                .ok_or_else(|| anyhow!("--heap-store s3 requires --s3-bucket"))?;
            if let Some(cache_dir) = config.cache_dir.clone() {
                tracing::info!(
                    "Heap: S3 bucket '{}' with write-through cache at {}",
                    bucket,
                    cache_dir
                );
                Ok(AnyHeapStorage::S3WithFsCache(
                    WriteThroughCacheHeapStorage::new(S3HeapStorage::new(bucket).await, cache_dir),
                ))
            } else {
                tracing::info!("Heap: S3 bucket '{}'", bucket);
                Ok(AnyHeapStorage::S3(S3HeapStorage::new(bucket).await))
            }
        }
        StoreKind::Dir => {
            let dir = config
                .heap_dir
                .clone()
                .unwrap_or_else(|| "/tmp/mcp-v8-heaps".to_string());
            tracing::info!("Heap: directory store at {}", dir);
            Ok(AnyHeapStorage::File(FileHeapStorage::new(dir)))
        }
        StoreKind::None => bail!("heap storage requested while heap persistence is disabled"),
    }
}

async fn attach_session_log(
    engine: Engine,
    config: &StorageBootstrapConfig,
    cluster_node: Option<&Arc<ClusterNode>>,
) -> Result<Engine> {
    if !config.heap_enabled() && !config.fs_enabled() {
        return Ok(engine);
    }
    let log = match SessionLog::new(&config.session_db_path) {
        Ok(log) => log,
        Err(error) => {
            tracing::warn!(
                "Failed to open session log at {}: {}. Session logging disabled.",
                config.session_db_path,
                error
            );
            return Ok(engine);
        }
    };
    tracing::info!("Session log opened at {}", config.session_db_path);
    let log = if let Some(cluster_node) = cluster_node {
        tracing::info!("Session log will use Raft cluster for replication");
        log.with_cluster(cluster_node.clone())
    } else {
        log
    };
    if let Some(from) = config.session_fork_from.as_deref() {
        fork_session(&log, from, config.session_id.as_deref()).await?;
    }
    Ok(engine.with_session_log(log))
}

fn attach_heap_tags(
    engine: Engine,
    config: &StorageBootstrapConfig,
    cluster_node: Option<&Arc<ClusterNode>>,
) -> Engine {
    if !config.heap_enabled() {
        return engine;
    }
    let path = format!("{}/heap-tags", config.session_db_path);
    match HeapTagStore::new(&path) {
        Ok(store) => {
            tracing::info!("Heap tag store opened at {}", path);
            let store = if let Some(cluster_node) = cluster_node {
                store.with_cluster(cluster_node.clone())
            } else {
                store
            };
            engine.with_heap_tag_store(store)
        }
        Err(error) => {
            tracing::warn!(
                "Failed to open heap tag store at {}: {}. Heap tagging disabled.",
                path,
                error
            );
            engine
        }
    }
}

async fn attach_filesystem(
    engine: Engine,
    config: &StorageBootstrapConfig,
    cluster_node: Option<&Arc<ClusterNode>>,
) -> Result<Engine> {
    if !config.fs_enabled() {
        return Ok(engine);
    }
    let store_dir = config
        .fs_dir
        .clone()
        .unwrap_or_else(|| format!("{}/fs-blobs", config.session_db_path));
    let labels_db = config
        .fs_labels_db
        .clone()
        .unwrap_or_else(|| format!("{}/fs-labels", config.session_db_path));
    let backend: Arc<dyn HeapStorage> = if config.fs_store == StoreKind::S3 {
        let bucket = config
            .s3_bucket
            .clone()
            .ok_or_else(|| anyhow!("--fs-store s3 requires --s3-bucket"))?;
        if let Some(cache_dir) = &config.cache_dir {
            tracing::info!(
                "FS snapshots: shared S3 blob storage (bucket {}) with write-through cache at {}",
                bucket,
                cache_dir
            );
            Arc::new(WriteThroughCacheHeapStorage::new(
                S3HeapStorage::new(bucket.clone()).await,
                cache_dir.clone(),
            ))
        } else {
            tracing::info!("FS snapshots: shared S3 blob storage (bucket {})", bucket);
            Arc::new(S3HeapStorage::new(bucket).await)
        }
    } else {
        Arc::new(FileHeapStorage::new(&store_dir))
    };
    let store = Arc::new(FsStore::new(backend));
    match LabelStore::new(&labels_db) {
        Ok(labels) => {
            tracing::info!(
                "FS snapshots: ENABLED (blobs at {}, labels at {})",
                store_dir,
                labels_db
            );
            let labels = if let Some(cluster_node) = cluster_node {
                tracing::info!("FS label writes will route through the Raft cluster leader");
                labels.with_cluster(cluster_node.clone())
            } else {
                labels
            };
            Ok(engine.with_fs_snapshots(store, Arc::new(labels)))
        }
        Err(error) => {
            tracing::error!(
                "FS snapshots: failed to open label store at {}: {}. Disabled.",
                labels_db,
                error
            );
            Ok(engine)
        }
    }
}

fn attach_execution_registry(engine: Engine, config: &StorageBootstrapConfig) -> Engine {
    let path = config
        .execution_db_path
        .clone()
        .unwrap_or_else(|| match config.http_port {
            Some(port) => format!("{}/executions-{}", config.session_db_path, port),
            None => format!("{}/executions", config.session_db_path),
        });
    match ExecutionRegistry::new(&path) {
        Ok(registry) => {
            tracing::info!("Execution registry opened at {}", path);
            engine.with_execution_registry(Arc::new(registry))
        }
        Err(error) => {
            tracing::warn!(
                "Failed to open execution registry at {}: {}. Async execution disabled.",
                path,
                error
            );
            engine
        }
    }
}

async fn fork_session(log: &SessionLog, from: &str, to: Option<&str>) -> Result<()> {
    let to = to.ok_or_else(|| {
        anyhow!("--session-fork-from requires --session-id (the new session to create)")
    })?;
    if from == to {
        bail!("--session-fork-from '{from}' must differ from --session-id '{to}'");
    }
    match log.fork(from, to).await.map_err(|error| anyhow!(error))? {
        ForkOutcome::Forked { heap, fs } => {
            tracing::info!(
                "Forked session '{to}' from '{from}' (heap {}, fs {:?})",
                heap,
                fs
            );
        }
        ForkOutcome::TargetExists => {
            tracing::info!("Session '{to}' already has history; not forking from '{from}'");
        }
        ForkOutcome::SourceEmpty => {
            tracing::warn!(
                "--session-fork-from '{from}' has no session history; '{to}' starts empty"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(data_dir: &std::path::Path) -> StorageBootstrapConfig {
        StorageBootstrapConfig {
            heap_store: StoreKind::None,
            heap_dir: None,
            fs_store: StoreKind::None,
            fs_dir: None,
            fs_labels_db: None,
            s3_bucket: None,
            cache_dir: None,
            session_db_path: data_dir.to_string_lossy().into_owned(),
            http_port: None,
            execution_db_path: None,
            heap_memory_max_bytes: 8 * 1024 * 1024,
            execution_timeout_secs: 30,
            max_concurrent_executions: 1,
            session_id: None,
            session_fork_from: None,
        }
    }

    #[tokio::test]
    async fn builds_stateless_registry_without_persistence() {
        let data_dir = tempfile::tempdir().unwrap();
        let library = build_storage_engine(config(data_dir.path()), None)
            .await
            .unwrap()
            .build();
        assert!(!library.session_capable());
        assert!(library.list_executions().is_ok());
    }

    #[tokio::test]
    async fn builds_independent_directory_heap_and_filesystem_axes() {
        let data_dir = tempfile::tempdir().unwrap();
        let mut config = config(data_dir.path());
        config.heap_store = StoreKind::Dir;
        config.heap_dir = Some(data_dir.path().join("heaps").to_string_lossy().into_owned());
        config.fs_store = StoreKind::Dir;
        let library = build_storage_engine(config, None).await.unwrap().build();
        assert!(library.heap_enabled());
        assert!(library.fs_enabled());
        assert!(library.session_capable());
    }

    #[tokio::test]
    async fn rejects_missing_s3_bucket_for_either_axis() {
        let data_dir = tempfile::tempdir().unwrap();
        let mut heap = config(data_dir.path());
        heap.heap_store = StoreKind::S3;
        assert!(build_storage_engine(heap, None).await.is_err());

        let mut fs = config(data_dir.path());
        fs.fs_store = StoreKind::S3;
        assert!(build_storage_engine(fs, None).await.is_err());
    }
}
