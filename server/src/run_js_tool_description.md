run javascript or typescript code in v8

TypeScript support is type removal only — types are stripped before execution, not checked. Invalid types will be silently removed, not reported as errors.

params:
- code: the javascript or typescript code to run
- heap (optional): content hash (SHA-256 hex string) from a previous execution to resume that session, or omit for a fresh session
- session (optional): a human-readable session name. When provided, a log entry is written recording the input heap, output heap, executed code, and timestamp. Use list_sessions and list_session_snapshots to browse session history.
- heap_memory_max_mb (optional): maximum V8 heap memory in megabytes (4–64, default: 8). Override the server default for this execution.
- execution_timeout_secs (optional): maximum execution time in seconds (1–300, default: 30). Override the server default for this execution.
- tags (optional): a JSON object of key-value string pairs to associate with the resulting heap snapshot. Tags can be used to label, categorize, or annotate heaps for later retrieval. Use get_heap_tags, set_heap_tags, delete_heap_tags, and query_heaps_by_tags to manage tags independently.

returns:
- output: the output of the javascript code
- heap: content hash of the new heap snapshot — pass this in the next call to resume the session



## Limitations

- **No `async`/`await` or Promises**: Asynchronous JavaScript is not supported. All code must be synchronous.
- **No `fetch` or network access by default**: When the server is started with `--opa-url`, a synchronous `fetch(url, opts?)` function becomes available. Each request is checked against an OPA policy before execution. The response object has `.ok`, `.status`, `.statusText`, `.url`, `.headers.get(name)`, `.text()`, and `.json()` methods. Without `--opa-url`, there is no network access.
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

In stateful mode, each execution returns a SHA-256 content hash for the heap snapshot — pass it back as the `heap` parameter in the next call to resume from that state. Omit `heap` for a fresh session. In stateless mode, no heap is returned and each execution starts fresh.
the source code of the runtime is this

```rust
/// Stateful V8 execution — runs V8 on the calling thread. Publishes an
/// IsolateHandle for external cancellation (e.g. async timeout in `run_js`).
/// Takes raw (already unwrapped) snapshot data. Returns (result, oom_flag).
pub fn execute_stateful(
    code: &str,
    raw_snapshot: Option<Vec<u8>>,
    heap_memory_max_bytes: usize,
    isolate_handle: Arc<Mutex<Option<v8::IsolateHandle>>>,
) -> (Result<(String, Vec<u8>, String), String>, bool) {
    let oom_flag = Arc::new(AtomicBool::new(false));

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

        // Publish handle immediately so caller can terminate us.
        *isolate_handle.lock().unwrap() = Some(snapshot_creator.thread_safe_handle());

        let cb_data_ptr = install_heap_limit_callback(&mut snapshot_creator, oom_flag.clone());

        let output_result;
        {
            let scope = &mut v8::HandleScope::new(&mut snapshot_creator);
            let context = v8::Context::new(scope, Default::default());
            let scope = &mut v8::ContextScope::new(scope, context);
            output_result = match eval(scope, code) {
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

        // Clear handle before snapshot_creator is consumed.
        *isolate_handle.lock().unwrap() = None;
        unsafe { let _ = Box::from_raw(cb_data_ptr); }

        let startup_data = snapshot_creator.create_blob(v8::FunctionCodeHandling::Clear)
            .ok_or("Failed to create V8 snapshot blob".to_string())?;
        let wrapped = wrap_snapshot(&startup_data);

        output_result.map(|output| (output, wrapped.data, wrapped.content_hash))
    }));

    let oom = oom_flag.load(Ordering::SeqCst);
    match result {
        Ok(Ok(triple)) => (Ok(triple), oom),
        Ok(Err(e)) => (Err(classify_termination_error(&oom_flag, false, e)), oom),
        Err(_panic) => (Err(classify_termination_error(
            &oom_flag, false, "V8 execution panicked unexpectedly".to_string(),
        )), oom),
    }
}

// Engine.run_js handles TypeScript stripping, timeout enforcement,
// concurrency control, heap storage, and session logging:
pub async fn run_js(
    &self,
    code: String,
    heap: Option<String>,
    session: Option<String>,
    heap_memory_max_mb: Option<usize>,
    execution_timeout_secs: Option<u64>,
) -> Result<JsResult, String> {
    // Strip TypeScript types before V8 execution (no-op for plain JS)
    let code = strip_typescript_types(&code)?;
    // ... acquires semaphore permit, loads snapshot from storage,
    // runs execute_stateful on blocking thread with async timeout,
    // saves resulting snapshot, logs to session if named ...
}
```
