//! Shared library facade used by embedded callers and all server transports.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value, json};

use crate::engine::execution::{
    ConsoleOutputPage, ExecutionInfo, ExecutionRegistry, ExecutionSummary,
};
use crate::engine::fs_labels::LabelStore;
use crate::engine::fs_store::FsStore;
use crate::engine::heap_storage::{AnyHeapStorage, FileHeapStorage};
use crate::engine::heap_tags::{HeapTagEntry, HeapTagStore};
use crate::engine::session_log::SessionLog;
use crate::engine::{
    Engine, FsLabelView, FsMergeResult, FsPushOutcome, FsRefLogView, MIN_HEAP_MEMORY_MB,
    RunJsRequest,
};
use crate::mcp::{ToolCatalog, built_in_tool_catalog};

const DEFAULT_HEAP_MEMORY_MB: usize = 64;
const DEFAULT_MAX_CONCURRENT_EXECUTIONS: usize = 4;

#[derive(Clone)]
pub struct McpJsRuntime {
    engine: Engine,
}

impl McpJsRuntime {
    pub fn new(engine: Engine) -> Self {
        Self { engine }
    }

    pub fn builder() -> McpJsRuntimeBuilder {
        McpJsRuntimeBuilder::default()
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

    pub fn instructions_override(&self) -> Option<Arc<str>> {
        self.engine.instructions_override()
    }

    pub fn run_js_description_override(&self) -> Option<Arc<str>> {
        self.engine.run_js_description_override()
    }

    pub fn upstream_mcp_stub_tools(&self) -> Vec<rmcp::model::Tool> {
        self.engine
            .mcp_client_manager()
            .map(|client| client.stub_tools())
            .unwrap_or_default()
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

    pub fn wasm_stub_tools(&self) -> Vec<rmcp::model::Tool> {
        self.engine.wasm_stub_tools()
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

    pub fn get_execution(&self, id: &str) -> Result<ExecutionInfo, String> {
        self.engine.get_execution(id)
    }

    pub fn get_execution_output(
        &self,
        id: &str,
        line_offset: Option<u64>,
        line_limit: Option<u64>,
        byte_offset: Option<u64>,
        byte_limit: Option<u64>,
    ) -> Result<ConsoleOutputPage, String> {
        self.engine
            .get_execution_output(id, line_offset, line_limit, byte_offset, byte_limit)
    }

    pub fn cancel_execution(&self, id: &str) -> Result<(), String> {
        self.engine.cancel_execution(id)
    }

    pub fn list_executions(&self) -> Result<Vec<ExecutionSummary>, String> {
        self.engine.list_executions()
    }

    pub async fn list_sessions(&self) -> Result<Vec<String>, String> {
        self.engine.list_sessions().await
    }

    pub async fn list_session_snapshots(
        &self,
        session: String,
        fields: Option<Vec<String>>,
    ) -> Result<Vec<Value>, String> {
        self.engine.list_session_snapshots(session, fields).await
    }

    pub async fn get_heap_tags(&self, heap: String) -> Result<HashMap<String, String>, String> {
        self.engine.get_heap_tags(heap).await
    }

    pub async fn set_heap_tags(
        &self,
        heap: String,
        tags: HashMap<String, String>,
    ) -> Result<(), String> {
        self.engine.set_heap_tags(heap, tags).await
    }

    pub async fn delete_heap_tags(
        &self,
        heap: String,
        keys: Option<Vec<String>>,
    ) -> Result<(), String> {
        self.engine.delete_heap_tags(heap, keys).await
    }

    pub async fn query_heaps_by_tags(
        &self,
        filter: HashMap<String, String>,
    ) -> Result<Vec<HeapTagEntry>, String> {
        self.engine.query_heaps_by_tags(filter).await
    }

    pub async fn fs_list_labels(&self) -> Result<Vec<FsLabelView>, String> {
        self.engine.fs_list_labels().await
    }

    pub async fn fs_resolve_label(&self, name: &str) -> Result<Option<String>, String> {
        self.engine.fs_resolve_label(name).await
    }

    pub async fn fs_set_label(
        &self,
        name: &str,
        ca_id: &str,
        message: Option<String>,
    ) -> Result<(), String> {
        self.engine.fs_set_label(name, ca_id, message).await
    }

    pub async fn fs_label_log(
        &self,
        name: &str,
        limit: Option<usize>,
    ) -> Result<Vec<FsRefLogView>, String> {
        self.engine.fs_label_log(name, limit).await
    }

    pub async fn fs_push(
        &self,
        label: &str,
        ca_id: &str,
        expected: Option<String>,
        force: bool,
        message: Option<String>,
    ) -> Result<FsPushOutcome, String> {
        self.engine
            .fs_push(label, ca_id, expected, force, message)
            .await
    }

    pub async fn fs_reset(
        &self,
        label: &str,
        ca_id: &str,
        allow_unlogged: bool,
        message: Option<String>,
    ) -> Result<(), String> {
        self.engine
            .fs_reset(label, ca_id, allow_unlogged, message)
            .await
    }

    pub async fn fs_merge(
        &self,
        ours: &str,
        theirs: &str,
        base: Option<String>,
        prefer: crate::engine::fs_merge::Prefer,
    ) -> Result<FsMergeResult, String> {
        self.engine.fs_merge(ours, theirs, base, prefer).await
    }

    pub fn tool_catalog(&self) -> ToolCatalog {
        built_in_tool_catalog(self.heap_enabled(), self.fs_enabled())
    }

    pub async fn shutdown(&self) -> (u64, u64) {
        self.engine.shutdown().await
    }

    pub async fn call_tool(
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
}

impl From<Engine> for McpJsRuntime {
    fn from(engine: Engine) -> Self {
        Self::new(engine)
    }
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
            heap_memory_max_mb: DEFAULT_HEAP_MEMORY_MB,
            execution_timeout_secs: crate::engine::DEFAULT_EXECUTION_TIMEOUT_SECS,
            max_concurrent_executions: DEFAULT_MAX_CONCURRENT_EXECUTIONS,
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

    pub fn build(self) -> Result<McpJsRuntime, String> {
        self.build_engine().map(McpJsRuntime::new)
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
