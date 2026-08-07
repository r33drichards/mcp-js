//! The engine's FFI surface: uniffi-exported records, error type, factory
//! functions, and the exported `impl Engine` block, plus the builder used by
//! Rust hosts. A child module of `engine`, so it works on the engine's own
//! fields directly — there is no wrapper type and no delegation layer.
//!
//! Everything here is re-exported from `crate::engine`.

use super::*;

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
            Self::InvalidJson { message, .. } => message,
        }
    }
}

#[uniffi::export]
impl Engine {
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

        let mut execution = self.run_js(request.code)
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
            .get_console_output(&execution_id, line_offset, line_limit, byte_offset, byte_limit)
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
            self
                .check_fs_snapshot_policy("label", Some(&name), Some(&ca_id))
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
            self
                .check_fs_snapshot_policy("push", Some(&label), Some(&ca_id))
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
            self
                .check_fs_snapshot_policy("reset", Some(&label), Some(&ca_id))
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
                        let ours_b = store
                            .read_file(oe)
                            .await
                            .map_err(|e| format!("fs_merge: read ours {}: {e}", c.path.display()))?;
                        let theirs_b = store
                            .read_file(te)
                            .await
                            .map_err(|e| format!("fs_merge: read theirs {}: {e}", c.path.display()))?;
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

impl Engine {
    /// Wrap a fully configured runtime for Rust transports without creating a
    /// second Tokio executor or crossing the FFI boundary.
    fn wrap(
        mut engine: Engine,
        tokio_runtime: Option<tokio::runtime::Runtime>,
        ephemeral_data_dir: Option<tempfile::TempDir>,
        cluster_node: Option<Arc<ClusterNode>>,
    ) -> Arc<Self> {
        engine.tokio_runtime = tokio_runtime.map(Arc::new);
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
        self
            .execution_registry
            .as_deref()
            .ok_or_else(|| operation_message("Execution registry not configured".to_string()))
    }

    fn session_log(&self) -> Result<&SessionLog, RuntimeError> {
        self
            .session_log
            .as_ref()
            .ok_or_else(|| operation_message("Session log not configured".to_string()))
    }

    fn heap_tag_store(&self) -> Result<&HeapTagStore, RuntimeError> {
        self
            .heap_tag_store
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
        self
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
            crate::mcp_dispatch::call_tool(self, session_id, mcp_headers, name, arguments)
                .await
        } else if name == "run_js" {
            crate::mcp_dispatch::run_js_blocking(self, mcp_headers, arguments).await
        } else {
            json!({ "error": format!("unknown stateless tool: {name}") })
        }
    }

    pub fn upstream_mcp_stub_call_response(
        &self,
        name: &str,
        arguments: Option<&serde_json::Map<String, Value>>,
    ) -> Option<rmcp::model::CallToolResult> {
        self
            .mcp_client_manager()
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
            .invoke_tool(ToolCallRequest {
                name: "run_js".to_string(),
                arguments_json: r#"{"code":"console.log(6 * 7)"}"#.to_string(),
                session_id: None,
                mcp_headers: None,
            })
            .await
            .unwrap();
        let value: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(value["output"], "42");
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
