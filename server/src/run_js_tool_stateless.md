run javascript code in v8

params:
- code: the javascript code to run
- heap_memory_max_mb (optional): maximum V8 heap memory in megabytes (1–64, default: 8). Override the server default for this execution.
- execution_timeout_secs (optional): maximum execution time in seconds (1–300, default: 30). Override the server default for this execution.

returns:
- output: the output of the javascript code




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


you are running in stateless mode, so the heap is not persisted between executions.
the source code of the runtime is this

```rust
// Execute JS in a stateless isolate (no snapshot creation)
pub fn execute_stateless(code: String, heap_memory_max_bytes: usize, timeout_secs: u64) -> Result<String, String> {
    let oom_flag = Arc::new(AtomicBool::new(false));
    let timeout_flag = Arc::new(AtomicBool::new(false));

    let result = catch_unwind(AssertUnwindSafe(|| {
        let params = create_params_with_heap_limit(heap_memory_max_bytes);
        let mut isolate = v8::Isolate::new(params);

        let cb_data_ptr = install_heap_limit_callback(&mut isolate, oom_flag.clone());
        let _timeout_guard = install_execution_timeout(&mut isolate, timeout_secs, timeout_flag.clone());

        let eval_result = {
            let scope = &mut v8::HandleScope::new(&mut isolate);
            let context = v8::Context::new(scope, Default::default());
            let scope = &mut v8::ContextScope::new(scope, context);

            match eval(scope, &code) {
                Ok(value) => match value.to_string(scope) {
                    Some(s) => Ok(s.to_rust_string_lossy(scope)),
                    None => Err("Failed to convert result to string".to_string()),
                },
                Err(e) => Err(e),
            }
        };

        drop(isolate);
        unsafe { let _ = Box::from_raw(cb_data_ptr); }

        eval_result
    }));

    match result {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(e)) => Err(classify_termination_error(&oom_flag, &timeout_flag, e)),
        Err(_panic) => Err(classify_termination_error(
            &oom_flag,
            &timeout_flag,
            "V8 execution panicked unexpectedly".to_string(),
        )),
    }
}

// Stateless service implementation
#[tool(tool_box)]
impl StatelessService {
    pub fn new(heap_memory_max_bytes: usize, execution_timeout_secs: u64) -> Self {
        Self { heap_memory_max_bytes, execution_timeout_secs }
    }

    #[tool(description = include_str!("run_js_tool_stateless.md"))]
    pub async fn run_js(
        &self,
        #[tool(param)] code: String,
        #[tool(param)]
        #[serde(default)]
        heap_memory_max_mb: Option<usize>,
        #[tool(param)]
        #[serde(default)]
        execution_timeout_secs: Option<u64>,
    ) -> RunJsStatelessResponse {
        let max_bytes = heap_memory_max_mb
            .map(|mb| mb * 1024 * 1024)
            .unwrap_or(self.heap_memory_max_bytes);
        let timeout = execution_timeout_secs.unwrap_or(self.execution_timeout_secs);
        let v8_result = tokio::task::spawn_blocking(move || execute_stateless(code, max_bytes, timeout)).await;

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
```