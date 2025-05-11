use rmcp::{
    model::{ServerCapabilities, ServerInfo},

    Error as McpError, RoleServer, ServerHandler, model::*, schemars,
    service::RequestContext, tool,
};
use serde_json::json;


use std::sync::Once;
use v8::{self};

pub(crate) mod heap_storage;
use crate::mcp::heap_storage::{HeapStorage, AnyHeapStorage};




fn eval<'s>(scope: &mut v8::HandleScope<'s>, code: &str) -> Result<v8::Local<'s, v8::Value>, String> {
    let scope = &mut v8::EscapableHandleScope::new(scope);
    let source = v8::String::new(scope, code).ok_or("Failed to create V8 string")?;
    let script = v8::Script::compile(scope, source, None).ok_or("Failed to compile script")?;
    let r = script.run(scope).ok_or("Failed to run script")?;
    Ok(scope.escape(r))
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

#[derive(Clone)]
pub struct GenericService {
    heap_storage: AnyHeapStorage,
}

// response to run_js
#[derive(Debug, Clone)]
pub struct RunJsResponse {
    pub output: String,
    pub heap: String,
}

impl IntoContents for RunJsResponse {
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

#[tool(tool_box)]
impl GenericService {
    pub async fn new(
        heap_storage: AnyHeapStorage,
    ) -> Self {
        Self {
            heap_storage,
        }
    }

    #[tool(description = "run javascript code in v8\n\nparams:\n- code: the javascript code to run\n- heap: the path to the heap file\n\nreturns:\n- output: the output of the javascript code\n- heap: the path to the heap file\n\nyou must send a heap file to the client. \n\n\nThe way the runtime works, is that there is no console.log. If you want the results of an execution, you must return it in the last line of code. \n\neg:\n\n```js\nconst result = 1 + 1;\nresult;\n```\n\nwould return:\n\n```\n2\n```\n\n")]
    pub async fn run_js(&self, #[tool(param)] code: String, #[tool(param)] heap: String) -> RunJsResponse {
        let snapshot = self.heap_storage.get(&heap).await.ok();
        let code_clone = code.clone();
        let v8_result = tokio::task::spawn_blocking(move || {
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
            let output = (|| {
                let output;
                {
                    let scope = &mut v8::HandleScope::new(&mut snapshot_creator);
                    let context = v8::Context::new(scope, Default::default());
                    let scope = &mut v8::ContextScope::new(scope, context);
                    let result = eval(scope, &code_clone)?;
                    let result_str = result
                        .to_string(scope)
                        .ok_or_else(|| "Failed to convert result to string".to_string())?;
                    output = result_str.to_rust_string_lossy(scope);
                    scope.set_default_context(context);
                } // All borrows of snapshot_creator end here
                let startup_data = snapshot_creator
                    .create_blob(v8::FunctionCodeHandling::Clear)
                    .ok_or("Failed to create V8 snapshot blob")?;
                let startup_data_vec = startup_data.to_vec();
                Ok::<_, String>((output, startup_data_vec))
            })();
            output
        }).await;
        match v8_result {
            Ok(Ok((output, startup_data))) => {
                if let Err(e) = self.heap_storage.put(&heap, &startup_data).await {
                    return RunJsResponse {
                        output: format!("Error saving heap: {}", e),
                        heap,
                    };
                }
                RunJsResponse { output, heap }
            }
            Ok(Err(e)) => RunJsResponse {
                output: format!("V8 error: {}", e),
                heap,
            },
            Err(e) => RunJsResponse {
                output: format!("Task join error: {}", e),
                heap,
            },
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