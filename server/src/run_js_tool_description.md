run javascript code in v8

params:
- code: the javascript code to run
- heap: the path to the heap file

returns:
- output: the output of the javascript code
- heap: the path to the heap file

you must send a heap file to the client.



## Limitations

While `mcp-v8` provides a powerful and persistent JavaScript execution environment, there are limitations to its runtime. 

- **No `async`/`await` or Promises**: Asynchronous JavaScript is not supported. All code must be synchronous.
- **No `fetch` or network access**: There is no built-in way to make HTTP requests or access the network.
- **No `console.log` or standard output**: Output from `console.log` or similar functions will not appear. To return results, ensure the value you want is the last line of your code.
- **No file system access**: The runtime does not provide access to the local file system or environment variables.
- **No `npm install` or external packages**: You cannot install or import npm packages. Only standard JavaScript (ECMAScript) built-ins are available.
- **No timers**: Functions like `setTimeout` and `setInterval` are not available.
- **No DOM or browser APIs**: This is not a browser environment; there is no access to `window`, `document`, or other browser-specific objects.


The way the runtime works, is that there is no console.log. If you want the results of an execution, you must return it in the last line of code.


eg:

```js
const result = 1 + 1;
result;
```

would return:

```
2
```

you must also jsonify an object, and return it as a string to see its content.

eg:

```js
const obj = {
  a: 1,
  b: 2,
};
JSON.stringify(obj);
```

would return:

```
{"a":1,"b":2}
```

you are running in stateful mode, so the heap is persisted between executions.
the source code of the runtime is this 
```rust
use rmcp::{
    model::{ServerCapabilities, ServerInfo},

    Error as McpError, RoleServer, ServerHandler, model::*,
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

// Stateful service implementation
#[tool(tool_box)]
impl StatefulService {
    pub fn new(heap_storage: AnyHeapStorage) -> Self {
        Self { heap_storage }
    }

    /// Execute JavaScript code with heap persistence. The heap parameter identifies the execution context.
    #[tool(description = include_str!("run_js_tool_description.md"))]
    pub async fn run_js(&self, #[tool(param)] code: String, #[tool(param)] heap: String) -> RunJsStatefulResponse {
        let snapshot = self.heap_storage.get(&heap).await.ok();
        let v8_result = tokio::task::spawn_blocking(move || execute_stateful(code, snapshot)).await;

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
```