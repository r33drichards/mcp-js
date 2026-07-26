use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::engine::execution::{
    ConsoleOutputPage as EngineConsoleOutputPage, ExecutionInfo as EngineExecutionInfo,
    ExecutionSummary as EngineExecutionSummary,
};
use crate::engine::mcp_client::McpClientManager;
use crate::engine::{
    Engine, FsLabelView, FsMergeResult, FsPushOutcome, FsRefLogView, RunJsRequest, initialize_v8,
};
use crate::runtime::McpJsRuntime;
use serde::Serialize;
use serde_json::Value;

const DEFAULT_HEAP_MEMORY_MB: u64 = 64;
const DEFAULT_EXECUTION_TIMEOUT_SECS: u64 = 30;
const DEFAULT_MAX_CONCURRENT_EXECUTIONS: u32 = 4;

#[derive(Clone, Copy, Debug, uniffi::Enum)]
pub enum LibraryMode {
    Stateless,
    LocalStateful,
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct LibraryConfig {
    pub mode: LibraryMode,
    pub data_dir: Option<String>,
    pub heap_memory_max_mb: u64,
    pub execution_timeout_secs: u64,
    pub max_concurrent_executions: u32,
}

impl Default for LibraryConfig {
    fn default() -> Self {
        Self {
            mode: LibraryMode::Stateless,
            data_dir: None,
            heap_memory_max_mb: DEFAULT_HEAP_MEMORY_MB,
            execution_timeout_secs: DEFAULT_EXECUTION_TIMEOUT_SECS,
            max_concurrent_executions: DEFAULT_MAX_CONCURRENT_EXECUTIONS,
        }
    }
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct ToolDefinition {
    pub name: String,
    pub description: Option<String>,
    pub input_schema_json: String,
}

#[derive(Clone, Debug, Serialize, uniffi::Record)]
pub struct LibraryCapabilities {
    pub heap: bool,
    pub filesystem: bool,
    pub sessions: bool,
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
    _ephemeral_data_dir: Option<tempfile::TempDir>,
}

#[uniffi::export]
pub fn default_library_config() -> LibraryConfig {
    LibraryConfig::default()
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
        Ok(Arc::new(Self {
            tokio_runtime: Some(tokio_runtime),
            runtime,
            _ephemeral_data_dir: ephemeral_data_dir,
        }))
    }

    pub fn mode(&self) -> LibraryMode {
        if self.runtime.session_capable() {
            LibraryMode::LocalStateful
        } else {
            LibraryMode::Stateless
        }
    }

    pub fn capabilities(&self) -> LibraryCapabilities {
        LibraryCapabilities {
            heap: self.runtime.heap_enabled(),
            filesystem: self.runtime.fs_enabled(),
            sessions: self.runtime.session_capable(),
        }
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

    pub fn list_tools(&self) -> Result<Vec<ToolDefinition>, LibraryError> {
        self.runtime
            .tool_catalog()
            .tools
            .into_iter()
            .map(|tool| {
                let input_schema_json =
                    serde_json::to_string(&tool.input_schema).map_err(|error| {
                        LibraryError::Initialization {
                            message: format!(
                                "failed to serialize schema for '{}': {error}",
                                tool.name
                            ),
                        }
                    })?;
                Ok(ToolDefinition {
                    name: tool.name,
                    description: tool.description,
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
        mcp_headers_json: Option<String>,
    ) -> Result<String, LibraryError> {
        let arguments = parse_json_object("arguments_json", &arguments_json)?;
        let mcp_headers = mcp_headers_json
            .as_deref()
            .map(|json| parse_json_object("mcp_headers_json", json))
            .transpose()?;

        let tokio_runtime =
            self.tokio_runtime
                .as_ref()
                .ok_or_else(|| LibraryError::Initialization {
                    message: "synchronous tool calls require a library-created runtime".to_string(),
                })?;
        let result = tokio_runtime.block_on(self.runtime.call_tool(
            session_id.as_deref(),
            mcp_headers.as_ref(),
            &name,
            &arguments,
        ));

        serde_json::to_string(&result).map_err(|error| LibraryError::ToolCall {
            message: format!("failed to serialize result: {error}"),
        })
    }
}

impl McpJsLibrary {
    /// Wrap a fully configured runtime for Rust transports without creating a
    /// second Tokio executor or crossing the FFI boundary.
    pub fn from_runtime(runtime: McpJsRuntime) -> Arc<Self> {
        Arc::new(Self {
            tokio_runtime: None,
            runtime,
            _ephemeral_data_dir: None,
        })
    }

    pub fn from_engine(engine: Engine) -> Arc<Self> {
        Self::from_runtime(McpJsRuntime::new(engine))
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

    pub fn mcp_client_manager(&self) -> Option<Arc<McpClientManager>> {
        self.runtime.mcp_client_manager()
    }

    pub fn wasm_stub_tools(&self) -> Vec<rmcp::model::Tool> {
        self.runtime.wasm_stub_tools()
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

    pub async fn fs_list_labels(&self) -> Result<Vec<FsLabelView>, String> {
        self.runtime.fs_list_labels().await
    }

    pub async fn fs_resolve_label(&self, name: &str) -> Result<Option<String>, String> {
        self.runtime.fs_resolve_label(name).await
    }

    pub async fn fs_set_label(
        &self,
        name: &str,
        ca_id: &str,
        message: Option<String>,
    ) -> Result<(), String> {
        self.runtime.fs_set_label(name, ca_id, message).await
    }

    pub async fn fs_label_log(
        &self,
        name: &str,
        limit: Option<usize>,
    ) -> Result<Vec<FsRefLogView>, String> {
        self.runtime.fs_label_log(name, limit).await
    }

    pub async fn fs_push(
        &self,
        label: &str,
        ca_id: &str,
        expected: Option<String>,
        force: bool,
        message: Option<String>,
    ) -> Result<FsPushOutcome, String> {
        self.runtime
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
        self.runtime
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
        self.runtime.fs_merge(ours, theirs, base, prefer).await
    }

    pub async fn call_tool_async(
        &self,
        session_id: Option<&str>,
        mcp_headers: Option<&Value>,
        name: &str,
        arguments: &Value,
    ) -> Value {
        self.runtime
            .call_tool(session_id, mcp_headers, name, arguments)
            .await
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
        .max_concurrent_executions(config.max_concurrent_executions as usize);
    let builder = match config.mode {
        LibraryMode::Stateless => builder.stateless(data_dir),
        LibraryMode::LocalStateful => builder.local_stateful(data_dir),
    };
    let runtime = builder.build().map_err(init_message)?;
    Ok((runtime, ephemeral_data_dir))
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
    fn stateless_run_js_executes_through_dispatcher() {
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

        let submitted: Value = serde_json::from_str(
            &library
                .call_tool(
                    "run_js".to_string(),
                    r#"{"code":"console.log(40 + 2)"}"#.to_string(),
                    Some("ffi-test".to_string()),
                    None,
                )
                .unwrap(),
        )
        .unwrap();
        let execution_id = submitted["execution_id"].as_str().unwrap();

        let mut completed = false;
        for _ in 0..200 {
            let status = library.get_execution(execution_id.to_string()).unwrap();
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
            .get_execution_output(execution_id.to_string(), None, None, None, None)
            .unwrap();
        assert_eq!(output.data, "42");
    }
}
