run javascript or typescript code in v8

TypeScript support is type removal only — types are stripped before execution, not checked. Invalid types will be silently removed, not reported as errors.

params:
- code: the javascript or typescript code to run
- heap_memory_max_mb (optional): maximum V8 heap memory in megabytes (1–64, default: 8). Override the server default for this execution.
- execution_timeout_secs (optional): maximum execution time in seconds (1–300, default: 30). Override the server default for this execution.

returns:
- output: the output of the javascript code



## Limitations

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
/// Stateless V8 execution — runs V8 on the calling thread. Publishes an
/// IsolateHandle for external cancellation (e.g. async timeout in `run_js`).
/// Returns (result, oom_flag).
pub fn execute_stateless(
    code: &str,
    heap_memory_max_bytes: usize,
    isolate_handle: Arc<Mutex<Option<v8::IsolateHandle>>>,
) -> (Result<String, String>, bool) {
    let oom_flag = Arc::new(AtomicBool::new(false));

    let result = catch_unwind(AssertUnwindSafe(|| {
        let params = create_params_with_heap_limit(heap_memory_max_bytes);
        let mut isolate = v8::Isolate::new(params);

        // Publish handle immediately so caller can terminate us.
        *isolate_handle.lock().unwrap() = Some(isolate.thread_safe_handle());

        let cb_data_ptr = install_heap_limit_callback(&mut isolate, oom_flag.clone());

        let eval_result = {
            let scope = &mut v8::HandleScope::new(&mut isolate);
            let context = v8::Context::new(scope, Default::default());
            let scope = &mut v8::ContextScope::new(scope, context);

            match eval(scope, code) {
                Ok(value) => match value.to_string(scope) {
                    Some(s) => Ok(s.to_rust_string_lossy(scope)),
                    None => Err("Failed to convert result to string".to_string()),
                },
                Err(e) => Err(e),
            }
        };

        // Clear handle BEFORE destroying isolate.
        *isolate_handle.lock().unwrap() = None;
        drop(isolate);
        unsafe { let _ = Box::from_raw(cb_data_ptr); }

        eval_result
    }));

    let oom = oom_flag.load(Ordering::SeqCst);
    match result {
        Ok(Ok(output)) => (Ok(output), oom),
        Ok(Err(e)) => (Err(classify_termination_error(&oom_flag, false, e)), oom),
        Err(_panic) => (Err(classify_termination_error(
            &oom_flag, false, "V8 execution panicked unexpectedly".to_string(),
        )), oom),
    }
}

// Engine.run_js handles TypeScript stripping, timeout enforcement,
// and concurrency control:
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
    // ... acquires semaphore permit, runs execute_stateless on
    // blocking thread with async timeout via tokio::select! ...
}
```
