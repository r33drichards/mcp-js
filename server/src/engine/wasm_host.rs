//! WASM host imports: filesystem, networking, and OPA policy middleware.
//!
//! Provides Rust-backed V8 functions (`host_read_file`, `host_write_file`,
//! `host_fetch`) that are passed as an import object to `WebAssembly.Instance`.
//! Every call is gated by an OPA (Rego) policy evaluated via `regorus`.
//!
//! ## ABI convention
//!
//! Host functions that return variable-length data call the WASM module's
//! exported `alloc(size) -> ptr`, write a length-prefixed result
//! (`[len:i32 LE][data:u8…]`), and return the pointer as `i32`. A return
//! of `0` signals an error retrievable via `host_last_error()`.
//!
//! WASM modules **must export**: `memory` (linear memory) and
//! `alloc(i32) -> i32` (bump allocator).

use std::cell::RefCell;

use serde::Serialize;

// ── Configuration ────────────────────────────────────────────────────────

/// Controls which host capabilities are available to WASM modules.
/// The `policy_engine` field holds a pre-loaded Rego policy; when `None`,
/// all host import calls are denied (default-deny).
#[derive(Clone, Debug, Default)]
pub struct WasmHostConfig {
    /// Pre-loaded regorus engine with the user's `.rego` policy.
    /// `None` means no policy ⇒ all host calls denied.
    pub policy_engine: Option<regorus::Engine>,
}

impl WasmHostConfig {
    /// Returns `true` when a policy is loaded, meaning host imports should
    /// be wired up during WASM instantiation.
    pub fn has_imports(&self) -> bool {
        self.policy_engine.is_some()
    }
}

// ── Per-module shared state ──────────────────────────────────────────────

/// Shared state accessible to all host import functions for a single WASM
/// module instance. Created during `inject_wasm_modules` and kept alive on
/// the caller's stack until V8 execution completes.
pub struct WasmHostState {
    /// The `WebAssembly.Memory` object (NOT the ArrayBuffer — that can be
    /// detached on `memory.grow()`). Re-extract `.buffer` on each call.
    pub memory_obj: RefCell<Option<v8::Global<v8::Object>>>,
    /// The WASM module's exported `alloc(i32) -> i32` function.
    pub alloc_fn: RefCell<Option<v8::Global<v8::Function>>>,
    /// Last error message from a failed host call.
    pub last_error: RefCell<Option<String>>,
    /// Reference to the shared OPA policy config.
    pub config: WasmHostConfig,
    /// Name of the WASM module (for policy input).
    pub module_name: String,
}

// ── OPA policy evaluation ────────────────────────────────────────────────

/// Input passed to the OPA policy on every host call.
#[derive(Serialize)]
pub struct PolicyInput<'a> {
    pub action: &'a str,
    pub module: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<&'a str>,
}

/// Evaluate `data.wasm.authz.allow` against the given input.
/// Returns `true` only if the policy explicitly evaluates to `true`.
fn check_policy(state: &WasmHostState, input: &PolicyInput) -> Result<bool, String> {
    let engine = match &state.config.policy_engine {
        Some(e) => e,
        None => return Ok(false), // no policy ⇒ deny
    };

    let mut engine = engine.clone();
    let input_json = serde_json::to_string(input).map_err(|e| e.to_string())?;
    engine
        .set_input(regorus::Value::from_json_str(&input_json).map_err(|e| e.to_string())?);
    match engine.eval_rule("data.wasm.authz.allow".to_string()) {
        Ok(v) => Ok(v == regorus::Value::from(true)),
        Err(_) => Ok(false), // undefined ⇒ deny
    }
}

// ── WASM memory helpers ──────────────────────────────────────────────────

/// Get the current `ArrayBuffer` backing `WebAssembly.Memory`. Must be
/// called fresh each time because `memory.grow()` can replace the buffer.
fn get_memory_buffer<'s>(
    scope: &mut v8::HandleScope<'s>,
    state: &WasmHostState,
) -> Result<v8::Local<'s, v8::ArrayBuffer>, String> {
    let mem_global = state.memory_obj.borrow();
    let mem_obj = mem_global
        .as_ref()
        .ok_or("WASM memory not initialised")?;
    let mem_obj = v8::Local::new(scope, mem_obj);

    let buffer_key =
        v8::String::new(scope, "buffer").ok_or("Failed to create 'buffer' key")?;
    let buffer = mem_obj
        .get(scope, buffer_key.into())
        .ok_or("Memory.buffer not found")?;
    v8::Local::<v8::ArrayBuffer>::try_from(buffer)
        .map_err(|_| "Memory.buffer is not an ArrayBuffer".into())
}

/// Read a UTF-8 string from WASM linear memory at `(ptr, len)`.
fn read_wasm_string(
    scope: &mut v8::HandleScope,
    state: &WasmHostState,
    ptr: i32,
    len: i32,
) -> Result<String, String> {
    let ab = get_memory_buffer(scope, state)?;
    let bs = ab.get_backing_store();
    let byte_length = bs.len();
    let start = ptr as usize;
    let end = start + len as usize;
    if end > byte_length {
        return Err(format!(
            "WASM memory access out of bounds: {}..{} > {}",
            start, end, byte_length
        ));
    }
    let data = bs.data().ok_or("WASM memory has no backing store data")?;
    let slice =
        unsafe { std::slice::from_raw_parts(data.as_ptr() as *const u8, byte_length) };
    String::from_utf8(slice[start..end].to_vec())
        .map_err(|e| format!("Invalid UTF-8 in WASM memory: {}", e))
}

/// Read raw bytes from WASM linear memory at `(ptr, len)`.
fn read_wasm_bytes(
    scope: &mut v8::HandleScope,
    state: &WasmHostState,
    ptr: i32,
    len: i32,
) -> Result<Vec<u8>, String> {
    let ab = get_memory_buffer(scope, state)?;
    let bs = ab.get_backing_store();
    let byte_length = bs.len();
    let start = ptr as usize;
    let end = start + len as usize;
    if end > byte_length {
        return Err(format!(
            "WASM memory access out of bounds: {}..{} > {}",
            start, end, byte_length
        ));
    }
    let data = bs.data().ok_or("WASM memory has no backing store data")?;
    let slice =
        unsafe { std::slice::from_raw_parts(data.as_ptr() as *const u8, byte_length) };
    Ok(slice[start..end].to_vec())
}

/// Write `data` into WASM memory as a length-prefixed buffer via the
/// module's exported `alloc()`. Returns the pointer to the start of the
/// allocated region (where the 4-byte LE length prefix lives).
fn write_length_prefixed_to_wasm(
    scope: &mut v8::HandleScope,
    state: &WasmHostState,
    data: &[u8],
) -> Result<i32, String> {
    let total = 4 + data.len();

    // Call alloc(total)
    let alloc_global = state.alloc_fn.borrow();
    let alloc_fn = alloc_global
        .as_ref()
        .ok_or("WASM module does not export 'alloc' function")?;
    let alloc_fn = v8::Local::new(scope, alloc_fn);

    let undefined = v8::undefined(scope).into();
    let len_val = v8::Integer::new(scope, total as i32).into();
    let ptr_val = alloc_fn
        .call(scope, undefined, &[len_val])
        .ok_or("alloc() call failed")?;
    let ptr = ptr_val
        .int32_value(scope)
        .ok_or("alloc() did not return an i32")?;

    // Write [len:i32 LE][data] into WASM memory
    let ab = get_memory_buffer(scope, state)?;
    let bs = ab.get_backing_store();
    let byte_length = bs.len();
    let start = ptr as usize;
    let end = start + total;
    if end > byte_length {
        return Err(format!(
            "WASM alloc returned ptr {} but memory is only {} bytes",
            ptr, byte_length
        ));
    }
    let store_data = bs.data().ok_or("WASM memory backing store has no data")?;
    unsafe {
        let dest = (store_data.as_ptr() as *mut u8).add(start);
        let len_bytes = (data.len() as i32).to_le_bytes();
        std::ptr::copy_nonoverlapping(len_bytes.as_ptr(), dest, 4);
        std::ptr::copy_nonoverlapping(data.as_ptr(), dest.add(4), data.len());
    }

    Ok(ptr)
}

// ── Host function callbacks ──────────────────────────────────────────────

/// Extract `&WasmHostState` from the `v8::External` data attached to a
/// function callback.
///
/// # Safety
/// The pointer is valid for the lifetime of the V8 isolate execution
/// because the `Pin<Box<WasmHostState>>` vec is kept alive in the caller
/// of `inject_wasm_modules`.
unsafe fn extract_state<'a>(
    args: &v8::FunctionCallbackArguments,
) -> &'a WasmHostState {
    let external = v8::Local::<v8::External>::try_from(args.data()).unwrap();
    unsafe { &*(external.value() as *const WasmHostState) }
}

/// `env.host_read_file(path_ptr, path_len) -> i32`
fn host_read_file(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let state = unsafe { extract_state(&args) };
    let path_ptr = args.get(0).int32_value(scope).unwrap_or(0);
    let path_len = args.get(1).int32_value(scope).unwrap_or(0);

    let result = (|| -> Result<i32, String> {
        let path = read_wasm_string(scope, state, path_ptr, path_len)?;

        let input = PolicyInput {
            action: "fs_read",
            module: &state.module_name,
            path: Some(&path),
            url: None,
            method: None,
        };
        if !check_policy(state, &input)? {
            return Err(format!("Policy denied fs_read for path '{}'", path));
        }

        let contents =
            std::fs::read(&path).map_err(|e| format!("read_file failed: {}", e))?;
        write_length_prefixed_to_wasm(scope, state, &contents)
    })();

    match result {
        Ok(ptr) => rv.set(v8::Integer::new(scope, ptr).into()),
        Err(e) => {
            *state.last_error.borrow_mut() = Some(e);
            rv.set(v8::Integer::new(scope, 0).into());
        }
    }
}

/// `env.host_write_file(path_ptr, path_len, data_ptr, data_len) -> i32`
fn host_write_file(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let state = unsafe { extract_state(&args) };
    let path_ptr = args.get(0).int32_value(scope).unwrap_or(0);
    let path_len = args.get(1).int32_value(scope).unwrap_or(0);
    let data_ptr = args.get(2).int32_value(scope).unwrap_or(0);
    let data_len = args.get(3).int32_value(scope).unwrap_or(0);

    let result = (|| -> Result<i32, String> {
        let path = read_wasm_string(scope, state, path_ptr, path_len)?;

        let input = PolicyInput {
            action: "fs_write",
            module: &state.module_name,
            path: Some(&path),
            url: None,
            method: None,
        };
        if !check_policy(state, &input)? {
            return Err(format!("Policy denied fs_write for path '{}'", path));
        }

        let data = read_wasm_bytes(scope, state, data_ptr, data_len)?;
        std::fs::write(&path, &data)
            .map_err(|e| format!("write_file failed: {}", e))?;
        Ok(0)
    })();

    match result {
        Ok(status) => rv.set(v8::Integer::new(scope, status).into()),
        Err(e) => {
            *state.last_error.borrow_mut() = Some(e);
            rv.set(v8::Integer::new(scope, -1).into());
        }
    }
}

/// `env.host_fetch(url_ptr, url_len) -> i32`
fn host_fetch(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let state = unsafe { extract_state(&args) };
    let url_ptr = args.get(0).int32_value(scope).unwrap_or(0);
    let url_len = args.get(1).int32_value(scope).unwrap_or(0);

    let result = (|| -> Result<i32, String> {
        let url = read_wasm_string(scope, state, url_ptr, url_len)?;

        let input = PolicyInput {
            action: "net_fetch",
            module: &state.module_name,
            path: None,
            url: Some(&url),
            method: Some("GET"),
        };
        if !check_policy(state, &input)? {
            return Err(format!("Policy denied net_fetch for url '{}'", url));
        }

        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Err(format!("Only HTTP(S) URLs allowed, got: {}", url));
        }

        let body = ureq::get(&url)
            .call()
            .map_err(|e| format!("fetch failed: {}", e))?
            .into_body()
            .read_to_vec()
            .map_err(|e| format!("fetch body read failed: {}", e))?;

        if body.len() > 16 * 1024 * 1024 {
            return Err("fetch response exceeds 16 MB limit".into());
        }

        write_length_prefixed_to_wasm(scope, state, &body)
    })();

    match result {
        Ok(ptr) => rv.set(v8::Integer::new(scope, ptr).into()),
        Err(e) => {
            *state.last_error.borrow_mut() = Some(e);
            rv.set(v8::Integer::new(scope, 0).into());
        }
    }
}

/// `env.host_last_error() -> i32`
fn host_last_error(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let state = unsafe { extract_state(&args) };
    let err = state.last_error.borrow().clone();
    match err {
        Some(msg) => {
            let result = write_length_prefixed_to_wasm(scope, state, msg.as_bytes());
            match result {
                Ok(ptr) => rv.set(v8::Integer::new(scope, ptr).into()),
                Err(_) => rv.set(v8::Integer::new(scope, 0).into()),
            }
        }
        None => rv.set(v8::Integer::new(scope, 0).into()),
    }
}

// ── Import object builder ────────────────────────────────────────────────

/// Build a V8 import object:
/// `{ env: { host_read_file, host_write_file, host_fetch, host_last_error } }`
///
/// Each function carries a `v8::External` pointer to the given `WasmHostState`.
pub fn build_import_object<'s>(
    scope: &mut v8::HandleScope<'s>,
    state_ptr: *const WasmHostState,
) -> Result<v8::Local<'s, v8::Object>, String> {
    let import_obj = v8::Object::new(scope);
    let env_obj = v8::Object::new(scope);

    let external =
        v8::External::new(scope, state_ptr as *mut std::ffi::c_void);

    macro_rules! register_host_fn {
        ($name:expr, $callback:ident) => {{
            let key = v8::String::new(scope, $name)
                .ok_or_else(|| format!("Failed to create key '{}'", $name))?;
            let tmpl = v8::FunctionTemplate::builder($callback)
                .data(external.into())
                .build(scope);
            let func = tmpl
                .get_function(scope)
                .ok_or_else(|| format!("Failed to build function '{}'", $name))?;
            env_obj.set(scope, key.into(), func.into());
        }};
    }

    register_host_fn!("host_read_file", host_read_file);
    register_host_fn!("host_write_file", host_write_file);
    register_host_fn!("host_fetch", host_fetch);
    register_host_fn!("host_last_error", host_last_error);

    let env_key =
        v8::String::new(scope, "env").ok_or("Failed to create 'env' key")?;
    import_obj.set(scope, env_key.into(), env_obj.into());

    Ok(import_obj)
}

// ── WASM injection with host imports ─────────────────────────────────────

use super::WasmModule;

/// Compile and instantiate WASM modules, optionally with host imports.
///
/// When `host_config.has_imports()` is `true`, each module is instantiated
/// with an import object containing the host functions gated by OPA policy.
/// Otherwise, the original zero-import path is used.
///
/// Host states are intentionally leaked (`Box::leak`) to avoid drop-ordering
/// issues with V8's HandleScope temporaries in Rust 2024 edition. Each state
/// is a few hundred bytes per WASM module — negligible compared to the V8
/// isolate (~4–64 MB) that is created and destroyed per request.
pub fn inject_wasm_modules_with_imports(
    scope: &mut v8::ContextScope<v8::HandleScope>,
    modules: &[WasmModule],
    host_config: &WasmHostConfig,
) -> Result<(), String> {
    if modules.is_empty() {
        return Ok(());
    }

    let global = scope.get_current_context().global(scope);

    // Look up WebAssembly.Instance constructor once.
    let wa_key = v8::String::new(scope, "WebAssembly")
        .ok_or("Failed to create 'WebAssembly' string")?;
    let wa_obj = global
        .get(scope, wa_key.into())
        .ok_or("WebAssembly not found on global")?;
    let wa_obj: v8::Local<v8::Object> = wa_obj
        .try_into()
        .map_err(|_| "WebAssembly is not an object")?;

    let instance_key = v8::String::new(scope, "Instance")
        .ok_or("Failed to create 'Instance' string")?;
    let instance_ctor = wa_obj
        .get(scope, instance_key.into())
        .ok_or("WebAssembly.Instance not found")?;
    let instance_ctor: v8::Local<v8::Function> = instance_ctor
        .try_into()
        .map_err(|_| "WebAssembly.Instance is not a function")?;

    let exports_key = v8::String::new(scope, "exports")
        .ok_or("Failed to create 'exports' string")?;

    for m in modules {
        let module_obj = v8::WasmModuleObject::compile(scope, &m.bytes)
            .ok_or_else(|| format!("Failed to compile WASM module '{}'", m.name))?;

        if host_config.has_imports() {
            // ── Path with host imports ───────────────────────────────
            let state = Box::new(WasmHostState {
                memory_obj: RefCell::new(None),
                alloc_fn: RefCell::new(None),
                last_error: RefCell::new(None),
                config: host_config.clone(),
                module_name: m.name.clone(),
            });
            let state_ptr: *const WasmHostState = &*state;

            let import_obj = build_import_object(scope, state_ptr)?;

            let instance = instance_ctor
                .new_instance(scope, &[module_obj.into(), import_obj.into()])
                .ok_or_else(|| {
                    format!(
                        "Failed to instantiate WASM module '{}' with imports",
                        m.name
                    )
                })?;

            let exports = instance
                .get(scope, exports_key.into())
                .ok_or_else(|| {
                    format!("Failed to get exports from WASM module '{}'", m.name)
                })?;
            let exports_obj: v8::Local<v8::Object> = exports
                .try_into()
                .map_err(|_| format!("Exports of '{}' is not an object", m.name))?;

            // Extract memory and alloc from exports
            let memory_key = v8::String::new(scope, "memory")
                .ok_or("Failed to create 'memory' key")?;
            if let Some(memory_val) = exports_obj.get(scope, memory_key.into()) {
                if let Ok(memory_obj) = v8::Local::<v8::Object>::try_from(memory_val) {
                    *state.memory_obj.borrow_mut() =
                        Some(v8::Global::new(scope, memory_obj));
                }
            }

            let alloc_key = v8::String::new(scope, "alloc")
                .ok_or("Failed to create 'alloc' key")?;
            if let Some(alloc_val) = exports_obj.get(scope, alloc_key.into()) {
                if let Ok(alloc_func) = v8::Local::<v8::Function>::try_from(alloc_val) {
                    *state.alloc_fn.borrow_mut() =
                        Some(v8::Global::new(scope, alloc_func));
                }
            }

            // Leak the state — it must outlive the V8 scope. See doc comment.
            Box::leak(state);

            // Bind exports as a global variable
            let name_key = v8::String::new(scope, &m.name).ok_or_else(|| {
                format!("Failed to create global name for '{}'", m.name)
            })?;
            global.set(scope, name_key.into(), exports);
        } else {
            // ── Original path: no imports ────────────────────────────
            let instance = instance_ctor
                .new_instance(scope, &[module_obj.into()])
                .ok_or_else(|| {
                    format!("Failed to instantiate WASM module '{}'", m.name)
                })?;

            let exports = instance
                .get(scope, exports_key.into())
                .ok_or_else(|| {
                    format!("Failed to get exports from WASM module '{}'", m.name)
                })?;

            let name_key = v8::String::new(scope, &m.name).ok_or_else(|| {
                format!("Failed to create global name for WASM module '{}'", m.name)
            })?;
            global.set(scope, name_key.into(), exports);
        }
    }

    Ok(())
}
