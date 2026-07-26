use std::path::PathBuf;
use std::sync::Arc;

use crate::engine::execution::{ConsoleOutputPage, ExecutionInfo, ExecutionSummary};
use crate::engine::mcp_client::McpClientManager;
use crate::engine::{
    Engine, FsLabelView, FsMergeResult, FsPushOutcome, FsRefLogView, RunJsRequest, initialize_v8,
};
use crate::runtime::McpJsRuntime;
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

    pub fn get_execution(&self, id: &str) -> Result<ExecutionInfo, String> {
        self.runtime.get_execution(id)
    }

    pub fn get_execution_output(
        &self,
        id: &str,
        line_offset: Option<u64>,
        line_limit: Option<u64>,
        byte_offset: Option<u64>,
        byte_limit: Option<u64>,
    ) -> Result<ConsoleOutputPage, String> {
        self.runtime
            .get_execution_output(id, line_offset, line_limit, byte_offset, byte_limit)
    }

    pub fn cancel_execution(&self, id: &str) -> Result<(), String> {
        self.runtime.cancel_execution(id)
    }

    pub fn list_executions(&self) -> Result<Vec<ExecutionSummary>, String> {
        self.runtime.list_executions()
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
            let status: Value = serde_json::from_str(
                &library
                    .call_tool(
                        "get_execution".to_string(),
                        format!(r#"{{"execution_id":"{execution_id}"}}"#),
                        None,
                        None,
                    )
                    .unwrap(),
            )
            .unwrap();
            if status["status"] == "completed" {
                completed = true;
                break;
            }
            if matches!(
                status["status"].as_str(),
                Some("failed" | "timed_out" | "cancelled")
            ) {
                panic!("stateful execution failed: {status}");
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        assert!(completed, "stateful execution did not complete");

        let output: Value = serde_json::from_str(
            &library
                .call_tool(
                    "get_execution_output".to_string(),
                    format!(r#"{{"execution_id":"{execution_id}"}}"#),
                    None,
                    None,
                )
                .unwrap(),
        )
        .unwrap();
        assert_eq!(output["data"], "42");
    }
}
