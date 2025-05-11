use rmcp::{
    model::{ServerCapabilities, ServerInfo},

    Error as McpError, RoleServer, ServerHandler, model::*, schemars,
    service::RequestContext, tool,
};
use serde_json::json;


use std::io::Write;
use std::sync::Once;
use v8::{self};

mod heap_storage;
use crate::mcp::heap_storage::{HeapStorage, FileHeapStorage};

fn eval<'s>(scope: &mut v8::HandleScope<'s>, code: &str) -> Option<v8::Local<'s, v8::Value>> {
    let scope = &mut v8::EscapableHandleScope::new(scope);
    let source = v8::String::new(scope, code).unwrap();
    let script = v8::Script::compile(scope, source, None).unwrap();
    let r = script.run(scope);
    r.map(|v| scope.escape(v))
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

pub fn eval_js(code: &str, heap_name: &str, heap_storage: &dyn HeapStorage) -> Result<String, String> {
    let output;
    let startup_data = {
        let mut snapshot_creator = match heap_storage.get(heap_name) {
            Ok(snapshot) => {
                eprintln!("creating isolate from snapshot...");
                v8::Isolate::snapshot_creator_from_existing_snapshot(snapshot, None, None)
            }
            Err(_) => {
                eprintln!("snapshot not found, creating new isolate...");
                v8::Isolate::snapshot_creator(Default::default(), Default::default())
            }
        };
        {
            let scope = &mut v8::HandleScope::new(&mut snapshot_creator);
            let context = v8::Context::new(scope, Default::default());
            let scope = &mut v8::ContextScope::new(scope, context);

            let result = eval(scope, code).unwrap();

            let result_str = result
                .to_string(scope)
                .ok_or_else(|| "Failed to convert result to string".to_string())?;
            output = result_str.to_rust_string_lossy(scope);
            scope.set_default_context(context);
        }
        snapshot_creator
            .create_blob(v8::FunctionCodeHandling::Clear)
            .unwrap()
    };
    // Write snapshot to heap storage
    eprintln!("snapshot created");
    heap_storage.put(heap_name, &startup_data)?;
    Ok(output)
}

#[allow(dead_code)]
pub trait DataService: Send + Sync + 'static {
    fn get_data(&self) -> String;
    fn set_data(&mut self, data: String);
}

#[derive(Debug, Clone)]
pub struct MemoryDataService {
    data: String,
}

impl MemoryDataService {
    #[allow(dead_code)]
    pub fn new(initial_data: impl Into<String>) -> Self {
        Self {
            data: initial_data.into(),
        }
    }
}

impl DataService for MemoryDataService {
    fn get_data(&self) -> String {
        self.data.clone()
    }

    fn set_data(&mut self, data: String) {
        self.data = data;
    }
}

#[derive(Clone)]
pub struct GenericService {
    heap_storage: std::sync::Arc<dyn HeapStorage>,
}

// response to run_js
#[derive(Debug, Clone)]
pub struct RunJsResponse {
    pub output: String,
    pub heap: String,
}

impl IntoContents for RunJsResponse {
    fn into_contents(self) -> Vec<Content> {
        vec![Content::json(
            json!({
                "output": self.output,
                "heap": self.heap,
            })
        ).expect("failed to convert run_js response to content")]
    }
}

#[tool(tool_box)]
impl GenericService {
    pub fn new() -> Self {
        let heap_storage = std::sync::Arc::new(FileHeapStorage::new("/tmp/mcp_heap_storage"));
        Self {
            heap_storage,
        }
    }

    #[tool(description = "run javascript code in v8\n\nparams:\n- code: the javascript code to run\n- heap: the path to the heap file\n\nreturns:\n- output: the output of the javascript code\n- heap: the path to the heap file\n\nyou must send a heap file to the client. \n\n\nThe way the runtime works, is that there is no console.log. If you want the results of an execution, you must return it in the last line of code. \n\neg:\n\n```js\nconst result = 1 + 1;\nresult;\n```\n\nwould return:\n\n```\n2\n```\n\n")]
    pub async fn run_js(&self, #[tool(param)] code: String, #[tool(param)] heap: String) -> RunJsResponse {
        let output = eval_js(&code, &heap, self.heap_storage.as_ref()).unwrap();
        RunJsResponse {
            output,
            heap,
        }
    }


}

#[tool(tool_box)]
impl ServerHandler for GenericService {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some("generic data service".into()),
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