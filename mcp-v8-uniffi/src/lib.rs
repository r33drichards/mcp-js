use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;
use server::engine::execution::ExecutionRegistry;
use server::engine::heap_storage::{AnyHeapStorage, FileHeapStorage};
use server::engine::heap_tags::HeapTagStore;
use server::engine::session_log::SessionLog;
use server::engine::{Engine, initialize_v8};
use server::runtime::McpJsRuntime;

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
    tokio_runtime: tokio::runtime::Runtime,
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

        let (engine, ephemeral_data_dir) = build_engine(&config)?;
        Ok(Arc::new(Self {
            tokio_runtime,
            runtime: McpJsRuntime::new(engine),
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

        let result = self.tokio_runtime.block_on(self.runtime.call_tool(
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

fn validate_config(config: &LibraryConfig) -> Result<(), LibraryError> {
    if config.heap_memory_max_mb < server::engine::MIN_HEAP_MEMORY_MB as u64 {
        return Err(LibraryError::InvalidConfig {
            message: format!(
                "heap_memory_max_mb must be at least {}",
                server::engine::MIN_HEAP_MEMORY_MB
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

fn build_engine(
    config: &LibraryConfig,
) -> Result<(Engine, Option<tempfile::TempDir>), LibraryError> {
    let heap_memory_max_bytes = usize::try_from(config.heap_memory_max_mb)
        .ok()
        .and_then(|megabytes| megabytes.checked_mul(1024 * 1024))
        .ok_or_else(|| LibraryError::InvalidConfig {
            message: "heap_memory_max_mb is too large for this platform".to_string(),
        })?;
    let max_concurrent = config.max_concurrent_executions as usize;

    match config.mode {
        LibraryMode::Stateless => {
            let ephemeral_data_dir = if config.data_dir.is_none() {
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
            std::fs::create_dir_all(&data_dir)
                .map_err(|error| init_path_error(&data_dir, error))?;
            let registry = ExecutionRegistry::new(path_string(&data_dir.join("executions"))?)
                .map_err(init_message)?;
            let engine = Engine::new_stateless(
                heap_memory_max_bytes,
                config.execution_timeout_secs,
                max_concurrent,
            )
            .with_execution_registry(Arc::new(registry));
            Ok((engine, ephemeral_data_dir))
        }
        LibraryMode::LocalStateful => build_local_stateful_engine(
            config.data_dir.as_deref().expect("validated data_dir"),
            heap_memory_max_bytes,
            config.execution_timeout_secs,
            max_concurrent,
        )
        .map(|engine| (engine, None)),
    }
}

fn build_local_stateful_engine(
    data_dir: &str,
    heap_memory_max_bytes: usize,
    execution_timeout_secs: u64,
    max_concurrent: usize,
) -> Result<Engine, LibraryError> {
    let data_dir = PathBuf::from(data_dir);
    std::fs::create_dir_all(&data_dir).map_err(|error| init_path_error(&data_dir, error))?;

    let session_log =
        SessionLog::new(path_string(&data_dir.join("sessions"))?).map_err(init_message)?;
    let heap_tags =
        HeapTagStore::new(path_string(&data_dir.join("heap-tags"))?).map_err(init_message)?;
    let execution_registry =
        ExecutionRegistry::new(path_string(&data_dir.join("executions"))?).map_err(init_message)?;

    Ok(Engine::new_stateful(
        AnyHeapStorage::File(FileHeapStorage::new(data_dir.join("heaps"))),
        Some(session_log),
        Some(heap_tags),
        heap_memory_max_bytes,
        execution_timeout_secs,
        max_concurrent,
    )
    .with_execution_registry(Arc::new(execution_registry)))
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

fn path_string(path: &Path) -> Result<&str, LibraryError> {
    path.to_str().ok_or_else(|| LibraryError::Initialization {
        message: format!("path is not valid UTF-8: {}", path.display()),
    })
}

fn init_message(message: String) -> LibraryError {
    LibraryError::Initialization { message }
}

fn init_path_error(path: &Path, error: std::io::Error) -> LibraryError {
    LibraryError::Initialization {
        message: format!("failed to create '{}': {error}", path.display()),
    }
}

uniffi::setup_scaffolding!();

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
