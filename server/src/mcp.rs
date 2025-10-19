use rmcp::{
    model::{ServerCapabilities, ServerInfo, Resource, RawResource, ResourceContents, ListResourcesResult, ReadResourceResult, ReadResourceRequestParam},

    Error as McpError, RoleServer, ServerHandler, model::*,
    service::RequestContext, tool,
};
use serde_json::json;
use base64::Engine;


use std::sync::Once;
use v8::{self};

pub(crate) mod heap_storage;
use crate::mcp::heap_storage::AnyHeapStorage;




fn eval<'s>(scope: &mut v8::HandleScope<'s>, code: &str) -> Result<v8::Local<'s, v8::Value>, String> {
    let scope = &mut v8::EscapableHandleScope::new(scope);
    let source = v8::String::new(scope, code).ok_or("Failed to create V8 string")?;
    let script = v8::Script::compile(scope, source, None).ok_or("Failed to compile script")?;
    let r = script.run(scope).ok_or("Failed to run script")?;
    Ok(scope.escape(r))
}

// Execute JS in a stateless isolate (no snapshot creation)
fn execute_stateless(code: String) -> Result<String, String> {
    let isolate = &mut v8::Isolate::new(Default::default());
    let scope = &mut v8::HandleScope::new(isolate);
    let context = v8::Context::new(scope, Default::default());
    let scope = &mut v8::ContextScope::new(scope, context);

    let result = eval(scope, &code)?;
    match result.to_string(scope) {
        Some(s) => Ok(s.to_rust_string_lossy(scope)),
        None => Err("Failed to convert result to string".to_string()),
    }
}

// Execute JS with snapshot support (preserves heap state)
fn execute_stateful(code: String, snapshot: Option<Vec<u8>>) -> Result<(String, Vec<u8>), String> {
    let mut snapshot_creator = match snapshot {
        Some(snapshot) => {
            eprintln!("creating isolate from snapshot...");
            v8::Isolate::snapshot_creator_from_existing_snapshot(snapshot, None, None)
        }
        None => {
            eprintln!("snapshot not found, creating new isolate...");
            v8::Isolate::snapshot_creator(Default::default(), Default::default())
        }
    };

    let mut output_result: Result<String, String> = Err("Unknown error".to_string());
    {
        let scope = &mut v8::HandleScope::new(&mut snapshot_creator);
        let context = v8::Context::new(scope, Default::default());
        let scope = &mut v8::ContextScope::new(scope, context);
        let result = eval(scope, &code);
        match result {
            Ok(result) => {
                let result_str = result
                    .to_string(scope)
                    .ok_or_else(|| "Failed to convert result to string".to_string());
                match result_str {
                    Ok(s) => {
                        output_result = Ok(s.to_rust_string_lossy(scope));
                    }
                    Err(e) => {
                        output_result = Err(e);
                    }
                }
            }
            Err(e) => {
                output_result = Err(e);
            }
        }
        scope.set_default_context(context);
    }

    let startup_data = snapshot_creator.create_blob(v8::FunctionCodeHandling::Clear)
        .ok_or("Failed to create V8 snapshot blob".to_string())?;
    let startup_data_vec = startup_data.to_vec();

    output_result.map(|output| (output, startup_data_vec))
}

static INIT: Once = Once::new();
static mut PLATFORM: Option<v8::SharedRef<v8::Platform>> = None;

pub fn initialize_v8() {
    INIT.call_once(|| {
        let platform = v8::new_default_platform(0, false).make_shared();
        v8::V8::initialize_platform(platform.clone());
        v8::V8::initialize();
        unsafe {
            PLATFORM = Some(platform);
        }
    });
}

// Public wrapper for integration testing
// Note: This is exposed for integration tests and should not be used in production
pub fn execute_stateful_for_test(code: String, snapshot: Option<Vec<u8>>) -> Result<(String, Vec<u8>), String> {
    execute_stateful(code, snapshot)
}



#[allow(dead_code)]
pub trait DataService: Send + Sync + 'static {
    fn get_data(&self) -> String;
    fn set_data(&mut self, data: String);
}

// Stateful service with heap persistence
#[derive(Clone)]
pub struct StatefulService {
    heap_storage: AnyHeapStorage,
}

// Stateless service without heap persistence
#[derive(Clone)]
pub struct StatelessService;

// response to run_js (stateful - with heap)
#[derive(Debug, Clone)]
pub struct RunJsStatefulResponse {
    pub output: String,
    pub heap: String,
}

impl IntoContents for RunJsStatefulResponse {
    fn into_contents(self) -> Vec<Content> {
        match Content::json(json!({
            "output": self.output,
            "heap": self.heap,
        })) {
            Ok(content) => vec![content],
            Err(e) => vec![Content::text(format!("Failed to convert run_js response to content: {}", e))],
        }
    }
}

// response to run_js (stateless - no heap)
#[derive(Debug, Clone)]
pub struct RunJsStatelessResponse {
    pub output: String,
}

impl IntoContents for RunJsStatelessResponse {
    fn into_contents(self) -> Vec<Content> {
        match Content::json(json!({
            "output": self.output,
        })) {
            Ok(content) => vec![content],
            Err(e) => vec![Content::text(format!("Failed to convert run_js response to content: {}", e))],
        }
    }
}

// Stateless service implementation
#[tool(tool_box)]
impl StatelessService {
    pub fn new() -> Self {
        Self
    }

    /// Execute JavaScript code in a fresh, stateless V8 isolate. Each execution starts with a clean environment.
    #[tool(description = include_str!("run_js_tool_stateless.md"))]
    pub async fn run_js(&self, #[tool(param)] code: String) -> RunJsStatelessResponse {
        let v8_result = tokio::task::spawn_blocking(move || execute_stateless(code)).await;

        match v8_result {
            Ok(Ok(output)) => RunJsStatelessResponse { output },
            Ok(Err(e)) => RunJsStatelessResponse {
                output: format!("V8 error: {}", e),
            },
            Err(e) => RunJsStatelessResponse {
                output: format!("Task join error: {}", e),
            },
        }
    }
}

// response to create_heap
#[derive(Debug, Clone)]
pub struct CreateHeapResponse {
    pub heap_uri: String,
    pub message: String,
}

impl IntoContents for CreateHeapResponse {
    fn into_contents(self) -> Vec<Content> {
        match Content::json(json!({
            "heap_uri": self.heap_uri,
            "message": self.message,
        })) {
            Ok(content) => vec![content],
            Err(e) => vec![Content::text(format!("Failed to convert create_heap response to content: {}", e))],
        }
    }
}

// response to delete_heap
#[derive(Debug, Clone)]
pub struct DeleteHeapResponse {
    pub message: String,
}

impl IntoContents for DeleteHeapResponse {
    fn into_contents(self) -> Vec<Content> {
        match Content::json(json!({
            "message": self.message,
        })) {
            Ok(content) => vec![content],
            Err(e) => vec![Content::text(format!("Failed to convert delete_heap response to content: {}", e))],
        }
    }
}

// Stateful service implementation
#[tool(tool_box)]
impl StatefulService {
    pub fn new(heap_storage: AnyHeapStorage) -> Self {
        Self { heap_storage }
    }

    /// Execute JavaScript code with heap persistence. The heap_uri parameter must be a URI (e.g., file://my-heap or s3://my-heap)
    #[tool(description = include_str!("run_js_tool_description.md"))]
    pub async fn run_js(&self, #[tool(param)] code: String, #[tool(param)] heap_uri: String) -> RunJsStatefulResponse {
        // Check if heap exists
        match self.heap_storage.exists_by_uri(&heap_uri).await {
            Ok(false) => {
                return RunJsStatefulResponse {
                    output: format!("Heap '{}' does not exist. Use create_heap tool to create it first.", heap_uri),
                    heap: heap_uri,
                };
            }
            Err(e) => {
                return RunJsStatefulResponse {
                    output: format!("Error checking heap existence: {}", e),
                    heap: heap_uri,
                };
            }
            Ok(true) => {}
        }

        let snapshot = self.heap_storage.get_by_uri(&heap_uri).await.ok();
        let v8_result = tokio::task::spawn_blocking(move || execute_stateful(code, snapshot)).await;

        match v8_result {
            Ok(Ok((output, startup_data))) => {
                if let Err(e) = self.heap_storage.put_by_uri(&heap_uri, &startup_data).await {
                    return RunJsStatefulResponse {
                        output: format!("Error saving heap: {}", e),
                        heap: heap_uri,
                    };
                }
                RunJsStatefulResponse { output, heap: heap_uri }
            }
            Ok(Err(e)) => RunJsStatefulResponse {
                output: format!("V8 error: {}", e),
                heap: heap_uri,
            },
            Err(e) => RunJsStatefulResponse {
                output: format!("Task join error: {}", e),
                heap: heap_uri,
            },
        }
    }

    /// Create a new heap for storing JavaScript execution state. URI must be file://name or s3://name
    #[tool(description = "Create a new heap with the given URI (e.g., file://my-heap or s3://my-heap). Returns the heap URI that can be used with run_js.")]
    pub async fn create_heap(&self, #[tool(param)] heap_uri: String) -> CreateHeapResponse {
        // Check if heap already exists
        match self.heap_storage.exists_by_uri(&heap_uri).await {
            Ok(true) => {
                return CreateHeapResponse {
                    heap_uri: heap_uri.clone(),
                    message: format!("Heap '{}' already exists.", heap_uri),
                };
            }
            Err(e) => {
                return CreateHeapResponse {
                    heap_uri: String::new(),
                    message: format!("Error checking heap existence: {}", e),
                };
            }
            Ok(false) => {}
        }

        // Create an initial empty snapshot
        let v8_result = tokio::task::spawn_blocking(|| {
            execute_stateful("undefined".to_string(), None)
        }).await;

        match v8_result {
            Ok(Ok((_output, startup_data))) => {
                if let Err(e) = self.heap_storage.put_by_uri(&heap_uri, &startup_data).await {
                    return CreateHeapResponse {
                        heap_uri: String::new(),
                        message: format!("Error creating heap: {}", e),
                    };
                }
                CreateHeapResponse {
                    heap_uri: heap_uri.clone(),
                    message: format!("Successfully created heap '{}'", heap_uri),
                }
            }
            Ok(Err(e)) => CreateHeapResponse {
                heap_uri: String::new(),
                message: format!("V8 error creating heap: {}", e),
            },
            Err(e) => CreateHeapResponse {
                heap_uri: String::new(),
                message: format!("Task error creating heap: {}", e),
            },
        }
    }

    /// Delete an existing heap
    #[tool(description = "Delete an existing heap by URI (e.g., file://my-heap or s3://my-heap). This operation cannot be undone.")]
    pub async fn delete_heap(&self, #[tool(param)] heap_uri: String) -> DeleteHeapResponse {
        match self.heap_storage.delete_by_uri(&heap_uri).await {
            Ok(()) => DeleteHeapResponse {
                message: format!("Successfully deleted heap '{}'", heap_uri),
            },
            Err(e) => DeleteHeapResponse {
                message: format!("Error deleting heap '{}': {}", heap_uri, e),
            },
        }
    }
}

#[tool(tool_box)]
impl ServerHandler for StatelessService {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some("JavaScript execution service (stateless mode - no heap persistence)".into()),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }

    async fn initialize(
        &self,
        _request: InitializeRequestParam,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, McpError> {
        if let Some(http_request_part) = context.extensions.get::<axum::http::request::Parts>() {
            let initialize_headers = &http_request_part.headers;
            let initialize_uri = &http_request_part.uri;
            tracing::info!(?initialize_headers, %initialize_uri, "initialize from http server");
        }
        Ok(self.get_info())
    }
}

#[tool(tool_box)]
impl ServerHandler for StatefulService {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some("JavaScript execution service (stateful mode - with heap persistence)".into()),
            capabilities: ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
            ..Default::default()
        }
    }

    async fn initialize(
        &self,
        _request: InitializeRequestParam,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, McpError> {
        if let Some(http_request_part) = context.extensions.get::<axum::http::request::Parts>() {
            let initialize_headers = &http_request_part.headers;
            let initialize_uri = &http_request_part.uri;
            tracing::info!(?initialize_headers, %initialize_uri, "initialize from http server");
        }
        Ok(self.get_info())
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        // Get list of heaps from storage (returns URIs like file://name or s3://name)
        let heap_uris = self.heap_storage.list_all().await
            .map_err(|e| McpError::internal_error(format!("Failed to list heaps: {}", e), None))?;

        let resources: Vec<Resource> = heap_uris
            .into_iter()
            .map(|uri| {
                RawResource::new(
                    uri.clone(),
                    uri.clone(),
                ).no_annotation()
            })
            .collect();

        Ok(ListResourcesResult {
            resources,
            next_cursor: None,
        })
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, McpError> {
        let uri = request.uri;

        // Check if heap exists
        let exists = self.heap_storage.exists_by_uri(&uri).await
            .map_err(|e| McpError::internal_error(format!("Failed to check heap existence: {}", e), None))?;

        if !exists {
            return Err(McpError::resource_not_found(
                format!("Heap '{}' not found", uri),
                Some(json!({ "uri": uri }))
            ));
        }

        // Get the heap snapshot data
        let snapshot_data = self.heap_storage.get_by_uri(&uri).await
            .map_err(|e| McpError::internal_error(format!("Failed to read heap: {}", e), None))?;

        // Encode as base64 for binary data
        let blob = base64::engine::general_purpose::STANDARD.encode(&snapshot_data);

        Ok(ReadResourceResult {
            contents: vec![ResourceContents::BlobResourceContents {
                uri: uri.clone(),
                mime_type: Some("application/octet-stream".to_string()),
                blob,
            }],
        })
    }
}