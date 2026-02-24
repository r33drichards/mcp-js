run javascript or typescript code in v8

TypeScript support is type removal only — types are stripped before execution, not checked. Invalid types will be silently removed, not reported as errors.

params:
- code: the javascript or typescript code to run
- heap (optional): content hash from a previous execution to resume that session, or omit for a fresh session
- session (optional): a human-readable session name. When provided, a log entry is written recording the input heap, output heap, executed code, and timestamp. Use list_sessions and list_session_snapshots to browse session history.
- heap_memory_max_mb (optional): maximum V8 heap memory in megabytes (1–64, default: 8). Override the server default for this execution.
- execution_timeout_secs (optional): maximum execution time in seconds (1–300, default: 30). Override the server default for this execution.

returns:
- output: the output of the javascript code
- heap: content hash of the new heap snapshot — pass this in the next call to resume the session



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

you are running in stateful mode with content-addressed heap storage. Each execution returns a content hash for the heap snapshot — pass it back as the `heap` parameter in the next call to resume from that state. Omit `heap` for a fresh session.
the source code of the runtime is this

```rust
// Execute JS with snapshot support (preserves heap state)
pub fn execute_stateful(code: String, snapshot: Option<Vec<u8>>, heap_memory_max_bytes: usize, timeout_secs: u64) -> Result<(String, Vec<u8>, String), String> {
    let raw_snapshot = match snapshot {
        Some(data) if !data.is_empty() => Some(unwrap_snapshot(&data)?),
        _ => None,
    };

    let oom_flag = Arc::new(AtomicBool::new(false));
    let timeout_flag = Arc::new(AtomicBool::new(false));

    let result = catch_unwind(AssertUnwindSafe(|| {
        let params = Some(create_params_with_heap_limit(heap_memory_max_bytes));

        let mut snapshot_creator = match raw_snapshot {
            Some(raw) if !raw.is_empty() => {
                v8::Isolate::snapshot_creator_from_existing_snapshot(raw, None, params)
            }
            _ => {
                v8::Isolate::snapshot_creator(None, params)
            }
        };

        let cb_data_ptr = install_heap_limit_callback(&mut snapshot_creator, oom_flag.clone());
        let _timeout_guard = install_execution_timeout(&mut snapshot_creator, timeout_secs, timeout_flag.clone());

        let output_result;
        {
            let scope = &mut v8::HandleScope::new(&mut snapshot_creator);
            let context = v8::Context::new(scope, Default::default());
            let scope = &mut v8::ContextScope::new(scope, context);
            output_result = match eval(scope, &code) {
                Ok(result) => {
                    result
                        .to_string(scope)
                        .map(|s| s.to_rust_string_lossy(scope))
                        .ok_or_else(|| "Failed to convert result to string".to_string())
                }
                Err(e) => Err(e),
            };
            scope.set_default_context(context);
        }

        unsafe { let _ = Box::from_raw(cb_data_ptr); }

        let startup_data = snapshot_creator.create_blob(v8::FunctionCodeHandling::Clear)
            .ok_or("Failed to create V8 snapshot blob".to_string())?;
        let wrapped = wrap_snapshot(&startup_data);

        output_result.map(|output| (output, wrapped.data, wrapped.content_hash))
    }));

    match result {
        Ok(inner) => inner.map_err(|e| classify_termination_error(&oom_flag, &timeout_flag, e)),
        Err(_panic) => Err(classify_termination_error(
            &oom_flag,
            &timeout_flag,
            "V8 execution panicked unexpectedly".to_string(),
        )),
    }
}

// Stateful service with heap persistence
#[derive(Clone)]
pub struct StatefulService {
    heap_storage: AnyHeapStorage,
    session_log: Option<SessionLog>,
    heap_memory_max_bytes: usize,
    execution_timeout_secs: u64,
}

#[tool(tool_box)]
impl StatefulService {
    pub fn new(heap_storage: AnyHeapStorage, session_log: Option<SessionLog>, heap_memory_max_bytes: usize, execution_timeout_secs: u64) -> Self {
        Self { heap_storage, session_log, heap_memory_max_bytes, execution_timeout_secs }
    }

    #[tool(description = include_str!("run_js_tool_description.md"))]
    pub async fn run_js(
        &self,
        #[tool(param)] code: String,
        #[tool(param)]
        #[serde(default)]
        heap: Option<String>,
        #[tool(param)]
        #[serde(default)]
        session: Option<String>,
        #[tool(param)]
        #[serde(default)]
        heap_memory_max_mb: Option<usize>,
        #[tool(param)]
        #[serde(default)]
        execution_timeout_secs: Option<u64>,
    ) -> RunJsStatefulResponse {
        let max_bytes = heap_memory_max_mb
            .map(|mb| mb * 1024 * 1024)
            .unwrap_or(self.heap_memory_max_bytes);
        let timeout = execution_timeout_secs.unwrap_or(self.execution_timeout_secs);
        let snapshot = match &heap {
            Some(h) if !h.is_empty() => self.heap_storage.get(h).await.ok(),
            _ => None,
        };
        let code_for_log = code.clone();
        let v8_result = tokio::task::spawn_blocking(move || execute_stateful(code, snapshot, max_bytes, timeout)).await;

        match v8_result {
            Ok(Ok((output, startup_data, content_hash))) => {
                if let Err(e) = self.heap_storage.put(&content_hash, &startup_data).await {
                    return RunJsStatefulResponse {
                        output: format!("Error saving heap: {}", e),
                        heap: content_hash,
                    };
                }

                // Log to session if session name is provided and session log is available
                if let (Some(session_name), Some(log)) = (&session, &self.session_log) {
                    let entry = SessionLogEntry {
                        input_heap: heap.clone(),
                        output_heap: content_hash.clone(),
                        code: code_for_log,
                        timestamp: chrono::Utc::now().to_rfc3339(),
                    };
                    if let Err(e) = log.append(session_name, entry) {
                        tracing::warn!("Failed to log session entry: {}", e);
                    }
                }

                RunJsStatefulResponse { output, heap: content_hash }
            }
            Ok(Err(e)) => RunJsStatefulResponse {
                output: format!("V8 error: {}", e),
                heap: heap.unwrap_or_default(),
            },
            Err(e) => RunJsStatefulResponse {
                output: format!("Task join error: {}", e),
                heap: heap.unwrap_or_default(),
            },
        }
    }
}
```