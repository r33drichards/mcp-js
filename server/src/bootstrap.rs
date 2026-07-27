//! Runtime storage bootstrap shared by the CLI and embedded library hosts.

use std::sync::Arc;

use anyhow::{Result, anyhow, bail};

use crate::cli::StoreKind;
use crate::cluster::ClusterNode;
use crate::engine::Engine;
use crate::engine::execution::ExecutionRegistry;
use crate::engine::fs_labels::LabelStore;
use crate::engine::fs_store::FsStore;
use crate::engine::heap_storage::{
    AnyHeapStorage, FileHeapStorage, HeapStorage, S3HeapStorage, WriteThroughCacheHeapStorage,
};
use crate::engine::heap_tags::HeapTagStore;
use crate::engine::session_log::{ForkOutcome, SessionLog};
use crate::library::{LibraryError, LibraryFeatureConfig, McpJsLibrary};

pub struct LibraryBootstrap {
    engine: Engine,
    cluster_node: Option<Arc<ClusterNode>>,
}

impl LibraryBootstrap {
    pub fn with_feature_config(
        mut self,
        config: LibraryFeatureConfig,
    ) -> Result<Self, LibraryError> {
        if self.engine.heap_enabled() && !config.wasm_modules.is_empty() {
            return Err(LibraryError::InvalidConfig {
                message: "WASM modules are incompatible with heap persistence".to_string(),
            });
        }
        let wasm_default_max_bytes =
            usize::try_from(config.wasm_default_max_bytes).map_err(|_| {
                LibraryError::InvalidConfig {
                    message: "wasm_default_max_bytes is too large for this platform".to_string(),
                }
            })?;
        if config.wasm_stubs.prefix.is_empty() {
            return Err(LibraryError::InvalidConfig {
                message: "WASM stub prefix cannot be empty".to_string(),
            });
        }

        let mut names = std::collections::HashSet::new();
        let mut modules = Vec::with_capacity(config.wasm_modules.len());
        for module in config.wasm_modules {
            validate_wasm_module_name(&module.name)?;
            if !names.insert(module.name.clone()) {
                return Err(LibraryError::InvalidConfig {
                    message: format!("duplicate WASM module name: '{}'", module.name),
                });
            }
            let max_memory_bytes = module
                .max_memory_bytes
                .map(usize::try_from)
                .transpose()
                .map_err(|_| LibraryError::InvalidConfig {
                    message: format!(
                        "max_memory_bytes for WASM module '{}' is too large for this platform",
                        module.name
                    ),
                })?;
            modules.push(crate::engine::WasmModule {
                name: module.name,
                bytes: module.bytes,
                max_memory_bytes,
                description: module.description,
            });
        }

        self.engine = self
            .engine
            .with_wasm_default_max_bytes(wasm_default_max_bytes)
            .with_hardening(crate::engine::HardeningConfig {
                freeze_ops: config.hardening.freeze_ops,
                neutralize_proxy_details: config.hardening.neutralize_proxy_details,
                neutralize_introspection: config.hardening.neutralize_introspection,
                remove_bootstrap: config.hardening.remove_bootstrap,
                remove_shared_memory: config.hardening.remove_shared_memory,
            });
        if !modules.is_empty() {
            self.engine = self
                .engine
                .with_wasm_modules(modules)
                .with_wasm_stub_config(crate::engine::wasm_stub::WasmStubConfig {
                    prefix: config.wasm_stubs.prefix,
                    enabled: config.wasm_stubs.enabled,
                });
        }
        if let Some(text) = config.instructions_override {
            self.engine = self.engine.with_instructions_override(text);
        }
        if let Some(text) = config.run_js_description_override {
            self.engine = self.engine.with_run_js_description_override(text);
        }
        Ok(self)
    }

    pub fn with_fetch_config(mut self, config: crate::engine::fetch::FetchConfig) -> Self {
        self.engine = self.engine.with_fetch_config(config);
        self
    }

    pub fn with_fs_config(mut self, config: crate::engine::fs::FsConfig) -> Self {
        self.engine = self.engine.with_fs_config(config);
        self
    }

    pub fn with_fs_snapshot_policy(mut self, chain: Arc<crate::engine::opa::PolicyChain>) -> Self {
        self.engine = self.engine.with_fs_snapshot_policy(chain);
        self
    }

    pub fn with_module_loader_config(
        mut self,
        config: crate::engine::module_loader::ModuleLoaderConfig,
    ) -> Self {
        self.engine = self.engine.with_module_loader_config(config);
        self
    }

    pub fn with_subprocess_config(
        mut self,
        config: crate::engine::subprocess::SubprocessConfig,
    ) -> Self {
        self.engine = self.engine.with_subprocess_config(config);
        self
    }

    pub fn with_run_js_file_policy(
        mut self,
        policy: crate::engine::run_js_file::RunJsFilePolicy,
    ) -> Self {
        self.engine = self.engine.with_run_js_file_policy(policy);
        self
    }

    pub fn with_mcp_client_manager(
        mut self,
        manager: crate::engine::mcp_client::McpClientManager,
    ) -> Self {
        self.engine = self.engine.with_mcp_client_manager(manager);
        self
    }

    pub fn with_mcp_tools_policy_chain(
        mut self,
        chain: Arc<crate::engine::opa::PolicyChain>,
    ) -> Self {
        self.engine = self.engine.with_mcp_tools_policy_chain(chain);
        self
    }

    pub fn build(self) -> Arc<McpJsLibrary> {
        McpJsLibrary::from_engine_with_cluster(self.engine, self.cluster_node)
    }

    pub(crate) fn build_with_runtime(
        self,
        tokio_runtime: tokio::runtime::Runtime,
    ) -> Arc<McpJsLibrary> {
        McpJsLibrary::from_engine_with_tokio_runtime(self.engine, tokio_runtime, self.cluster_node)
    }
}

fn validate_wasm_module_name(name: &str) -> Result<(), LibraryError> {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return Err(LibraryError::InvalidConfig {
            message: "WASM module name cannot be empty".to_string(),
        });
    };
    if !first.is_ascii_alphabetic() && first != '_' && first != '$' {
        return Err(LibraryError::InvalidConfig {
            message: format!(
                "WASM module name '{}' must start with a letter, underscore, or dollar sign",
                name
            ),
        });
    }
    if let Some(invalid) = chars.find(|character| {
        !character.is_ascii_alphanumeric() && *character != '_' && *character != '$'
    }) {
        return Err(LibraryError::InvalidConfig {
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
) -> Result<LibraryBootstrap> {
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
    Ok(LibraryBootstrap {
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
