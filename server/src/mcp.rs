use rmcp::{
    model::{ServerCapabilities, ServerInfo},

    Error as McpError, RoleServer, ServerHandler, model::*,
    service::RequestContext, tool,
};
use serde_json::json;


use std::sync::Once;
use v8::{self};

pub(crate) mod heap_storage;
pub(crate) mod resource_storage;
use crate::mcp::heap_storage::{HeapStorage, AnyHeapStorage};
use crate::mcp::resource_storage::ResourceStorage;
use std::sync::Arc;




fn eval<'s>(scope: &mut v8::HandleScope<'s>, code: &str) -> Result<v8::Local<'s, v8::Value>, String> {
    let scope = &mut v8::EscapableHandleScope::new(scope);
    let source = v8::String::new(scope, code).ok_or("Failed to create V8 string")?;
    let script = v8::Script::compile(scope, source, None).ok_or("Failed to compile script")?;
    let r = script.run(scope).ok_or("Failed to run script")?;
    Ok(scope.escape(r))
}

// Struct to hold resource storage for V8 callbacks
struct IsolateData {
    runtime_handle: tokio::runtime::Handle,
    resource_storage: Arc<dyn ResourceStorage>,
}

// Helper to throw JavaScript errors
fn throw_error(scope: &mut v8::HandleScope, message: &str) {
    let error_msg = v8::String::new(scope, message).unwrap();
    let error = v8::Exception::error(scope, error_msg);
    scope.throw_exception(error);
}

// V8 callback for resource.read(uri)
fn resource_read_callback(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let isolate_data = match scope.get_slot::<Arc<IsolateData>>() {
        Some(data) => data.clone(),
        None => {
            throw_error(scope, "Internal error: isolate data not found");
            return;
        }
    };

    let uri = match args.get(0).to_string(scope) {
        Some(s) => s.to_rust_string_lossy(scope),
        None => {
            throw_error(scope, "resource.read(): URI string required as first argument");
            return;
        }
    };

    let storage = isolate_data.resource_storage.clone();
    let result = isolate_data.runtime_handle.block_on(async {
        storage.read(&uri).await
    });

    match result {
        Ok(content) => {
            let v8_str = v8::String::new(scope, &content).unwrap();
            rv.set(v8_str.into());
        }
        Err(e) => throw_error(scope, &format!("resource.read() failed: {}", e)),
    }
}

// V8 callback for resource.list(uri)
fn resource_list_callback(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let isolate_data = match scope.get_slot::<Arc<IsolateData>>() {
        Some(data) => data.clone(),
        None => {
            throw_error(scope, "Internal error: isolate data not found");
            return;
        }
    };

    let uri = match args.get(0).to_string(scope) {
        Some(s) => s.to_rust_string_lossy(scope),
        None => {
            throw_error(scope, "resource.list(): URI string required as first argument");
            return;
        }
    };

    let storage = isolate_data.resource_storage.clone();
    let result = isolate_data.runtime_handle.block_on(async {
        storage.list(&uri).await
    });

    match result {
        Ok(entries) => {
            let array = v8::Array::new(scope, entries.len() as i32);
            for (i, entry) in entries.iter().enumerate() {
                let v8_str = v8::String::new(scope, entry).unwrap();
                array.set_index(scope, i as u32, v8_str.into());
            }
            rv.set(array.into());
        }
        Err(e) => throw_error(scope, &format!("resource.list() failed: {}", e)),
    }
}

// V8 callback for resource.write(uri, content)
fn resource_write_callback(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let isolate_data = match scope.get_slot::<Arc<IsolateData>>() {
        Some(data) => data.clone(),
        None => {
            throw_error(scope, "Internal error: isolate data not found");
            return;
        }
    };

    let uri = match args.get(0).to_string(scope) {
        Some(s) => s.to_rust_string_lossy(scope),
        None => {
            throw_error(scope, "resource.write(): URI string required as first argument");
            return;
        }
    };

    let content = match args.get(1).to_string(scope) {
        Some(s) => s.to_rust_string_lossy(scope),
        None => {
            throw_error(scope, "resource.write(): content string required as second argument");
            return;
        }
    };

    let storage = isolate_data.resource_storage.clone();
    let result = isolate_data.runtime_handle.block_on(async {
        storage.write(&uri, &content).await
    });

    match result {
        Ok(()) => {
            let success = v8::Boolean::new(scope, true);
            rv.set(success.into());
        }
        Err(e) => throw_error(scope, &format!("resource.write() failed: {}", e)),
    }
}

// V8 callback for resource.delete(uri)
fn resource_delete_callback(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let isolate_data = match scope.get_slot::<Arc<IsolateData>>() {
        Some(data) => data.clone(),
        None => {
            throw_error(scope, "Internal error: isolate data not found");
            return;
        }
    };

    let uri = match args.get(0).to_string(scope) {
        Some(s) => s.to_rust_string_lossy(scope),
        None => {
            throw_error(scope, "resource.delete(): URI string required as first argument");
            return;
        }
    };

    let storage = isolate_data.resource_storage.clone();
    let result = isolate_data.runtime_handle.block_on(async {
        storage.delete(&uri).await
    });

    match result {
        Ok(()) => {
            let success = v8::Boolean::new(scope, true);
            rv.set(success.into());
        }
        Err(e) => throw_error(scope, &format!("resource.delete() failed: {}", e)),
    }
}

// Helper function to create a V8 context with resource functions
fn create_context_with_resources<'s>(
    scope: &mut v8::HandleScope<'s, ()>,
) -> v8::Local<'s, v8::Context> {
    // Create an object template for the global object
    let global_template = v8::ObjectTemplate::new(scope);

    // Create the 'resource' object template
    let resource_obj_template = v8::ObjectTemplate::new(scope);

    // Add resource methods
    let read_fn = v8::FunctionTemplate::new(scope, resource_read_callback);
    resource_obj_template.set(
        v8::String::new(scope, "read").unwrap().into(),
        read_fn.into(),
    );

    let list_fn = v8::FunctionTemplate::new(scope, resource_list_callback);
    resource_obj_template.set(
        v8::String::new(scope, "list").unwrap().into(),
        list_fn.into(),
    );

    let write_fn = v8::FunctionTemplate::new(scope, resource_write_callback);
    resource_obj_template.set(
        v8::String::new(scope, "write").unwrap().into(),
        write_fn.into(),
    );

    let delete_fn = v8::FunctionTemplate::new(scope, resource_delete_callback);
    resource_obj_template.set(
        v8::String::new(scope, "delete").unwrap().into(),
        delete_fn.into(),
    );

    // Add 'resource' to global
    global_template.set(
        v8::String::new(scope, "resource").unwrap().into(),
        resource_obj_template.into(),
    );

    // Create context with custom global template using ContextOptions
    let context_options = v8::ContextOptions {
        global_template: Some(global_template),
        ..Default::default()
    };
    v8::Context::new(scope, context_options)
}

// Execute JS in a stateless isolate (no snapshot creation)
fn execute_stateless(code: String, resource_storage: Arc<dyn ResourceStorage>) -> Result<String, String> {
    let isolate = &mut v8::Isolate::new(Default::default());

    // Set up isolate data for resource callbacks
    let runtime_handle = tokio::runtime::Handle::current();
    let isolate_data = Arc::new(IsolateData {
        runtime_handle,
        resource_storage,
    });
    isolate.set_slot(isolate_data);

    let scope = &mut v8::HandleScope::new(isolate);
    let context = create_context_with_resources(scope);
    let scope = &mut v8::ContextScope::new(scope, context);

    let result = eval(scope, &code)?;
    match result.to_string(scope) {
        Some(s) => Ok(s.to_rust_string_lossy(scope)),
        None => Err("Failed to convert result to string".to_string()),
    }
}

// Execute JS with snapshot support (preserves heap state)
fn execute_stateful(code: String, snapshot: Option<Vec<u8>>, resource_storage: Arc<dyn ResourceStorage>) -> Result<(String, Vec<u8>), String> {
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

    // Set up isolate data for resource callbacks
    let runtime_handle = tokio::runtime::Handle::current();
    let isolate_data = Arc::new(IsolateData {
        runtime_handle,
        resource_storage,
    });
    snapshot_creator.set_slot(isolate_data);

    let mut output_result: Result<String, String> = Err("Unknown error".to_string());
    {
        let scope = &mut v8::HandleScope::new(&mut snapshot_creator);
        let context = create_context_with_resources(scope);
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



#[allow(dead_code)]
pub trait DataService: Send + Sync + 'static {
    fn get_data(&self) -> String;
    fn set_data(&mut self, data: String);
}

// Stateful service with heap persistence
#[derive(Clone)]
pub struct StatefulService {
    heap_storage: AnyHeapStorage,
    resource_storage: Arc<dyn ResourceStorage>,
}

// Stateless service without heap persistence
#[derive(Clone)]
pub struct StatelessService {
    resource_storage: Arc<dyn ResourceStorage>,
}

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
    pub fn new(resource_storage: Arc<dyn ResourceStorage>) -> Self {
        Self { resource_storage }
    }

    /// Execute JavaScript code in a fresh, stateless V8 isolate. Each execution starts with a clean environment.
    #[tool(description = include_str!("run_js_tool_stateless.md"))]
    pub async fn run_js(&self, #[tool(param)] code: String) -> RunJsStatelessResponse {
        let resource_storage = self.resource_storage.clone();
        let v8_result = tokio::task::spawn_blocking(move || execute_stateless(code, resource_storage)).await;

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

// Stateful service implementation
#[tool(tool_box)]
impl StatefulService {
    pub fn new(heap_storage: AnyHeapStorage, resource_storage: Arc<dyn ResourceStorage>) -> Self {
        Self { heap_storage, resource_storage }
    }

    /// Execute JavaScript code with heap persistence. The heap parameter identifies the execution context.
    #[tool(description = include_str!("run_js_tool_description.md"))]
    pub async fn run_js(&self, #[tool(param)] code: String, #[tool(param)] heap: String) -> RunJsStatefulResponse {
        let snapshot = self.heap_storage.get(&heap).await.ok();
        let resource_storage = self.resource_storage.clone();
        let v8_result = tokio::task::spawn_blocking(move || execute_stateful(code, snapshot, resource_storage)).await;

        match v8_result {
            Ok(Ok((output, startup_data))) => {
                if let Err(e) = self.heap_storage.put(&heap, &startup_data).await {
                    return RunJsStatefulResponse {
                        output: format!("Error saving heap: {}", e),
                        heap,
                    };
                }
                RunJsStatefulResponse { output, heap }
            }
            Ok(Err(e)) => RunJsStatefulResponse {
                output: format!("V8 error: {}", e),
                heap,
            },
            Err(e) => RunJsStatefulResponse {
                output: format!("Task join error: {}", e),
                heap,
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