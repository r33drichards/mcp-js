//! Custom JavaScript functions defined in Rust
//!
//! This module provides a way to register custom Rust functions that can be called
//! from JavaScript code running in the V8 isolate.
//!
//! # Example
//!
//! ```rust
//! use server::custom_functions::{CustomFunction, execute_with_custom_functions};
//!
//! // Define a custom function that adds two numbers
//! fn add(args: &[serde_json::Value]) -> Result<serde_json::Value, String> {
//!     let a = args.get(0).and_then(|v| v.as_f64()).unwrap_or(0.0);
//!     let b = args.get(1).and_then(|v| v.as_f64()).unwrap_or(0.0);
//!     Ok(serde_json::json!(a + b))
//! }
//!
//! let functions = vec![
//!     CustomFunction::new("add", add),
//! ];
//!
//! let result = execute_with_custom_functions("add(2, 3)".to_string(), &functions);
//! assert_eq!(result.unwrap(), "5");
//! ```

use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::sync::Mutex;
use v8;

/// Type alias for custom function implementations
///
/// Custom functions receive a slice of JSON values as arguments and return
/// either a JSON value on success or an error string on failure.
pub type CustomFunctionImpl = fn(&[JsonValue]) -> Result<JsonValue, String>;

/// Represents a custom function that can be called from JavaScript
#[derive(Clone)]
pub struct CustomFunction {
    /// The name of the function as it will appear in JavaScript
    pub name: String,
    /// The Rust implementation of the function
    pub implementation: CustomFunctionImpl,
}

impl CustomFunction {
    /// Create a new custom function with the given name and implementation
    pub fn new(name: &str, implementation: CustomFunctionImpl) -> Self {
        Self {
            name: name.to_string(),
            implementation,
        }
    }
}

// Thread-local storage for custom function implementations during V8 execution
thread_local! {
    static CUSTOM_FUNCTIONS: Mutex<HashMap<String, CustomFunctionImpl>> = Mutex::new(HashMap::new());
}

/// Register custom functions for the current thread
fn register_functions(functions: &[CustomFunction]) {
    CUSTOM_FUNCTIONS.with(|cf| {
        let mut map = cf.lock().unwrap();
        map.clear();
        for func in functions {
            map.insert(func.name.clone(), func.implementation);
        }
    });
}

/// Clear registered custom functions
fn clear_functions() {
    CUSTOM_FUNCTIONS.with(|cf| {
        let mut map = cf.lock().unwrap();
        map.clear();
    });
}

/// Look up a custom function by name
fn get_function(name: &str) -> Option<CustomFunctionImpl> {
    CUSTOM_FUNCTIONS.with(|cf| {
        let map = cf.lock().unwrap();
        map.get(name).copied()
    })
}

/// The V8 callback that dispatches to the appropriate Rust function
fn custom_function_callback(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    // Get the function name from the data slot
    let data = args.data();
    let name = if let Some(name_str) = data.to_string(scope) {
        name_str.to_rust_string_lossy(scope)
    } else {
        rv.set(v8::undefined(scope).into());
        return;
    };

    // Look up the function implementation
    let func_impl = match get_function(&name) {
        Some(f) => f,
        None => {
            let error_msg = v8::String::new(scope, &format!("Function '{}' not found", name))
                .unwrap();
            let exception = v8::Exception::error(scope, error_msg);
            scope.throw_exception(exception);
            return;
        }
    };

    // Convert V8 arguments to JSON values
    let mut json_args = Vec::new();
    for i in 0..args.length() {
        let arg = args.get(i);
        let json_value = v8_to_json(scope, arg);
        json_args.push(json_value);
    }

    // Call the Rust function
    match func_impl(&json_args) {
        Ok(result) => {
            let v8_result = json_to_v8(scope, &result);
            rv.set(v8_result);
        }
        Err(e) => {
            let error_msg = v8::String::new(scope, &e).unwrap();
            let exception = v8::Exception::error(scope, error_msg);
            scope.throw_exception(exception);
        }
    }
}

/// Convert a V8 value to a JSON value
fn v8_to_json(scope: &mut v8::HandleScope, value: v8::Local<v8::Value>) -> JsonValue {
    if value.is_undefined() || value.is_null() {
        JsonValue::Null
    } else if value.is_boolean() {
        JsonValue::Bool(value.boolean_value(scope))
    } else if value.is_number() {
        let num = value.number_value(scope).unwrap_or(0.0);
        if num.fract() == 0.0 && num >= i64::MIN as f64 && num <= i64::MAX as f64 {
            JsonValue::Number(serde_json::Number::from(num as i64))
        } else {
            serde_json::Number::from_f64(num)
                .map(JsonValue::Number)
                .unwrap_or(JsonValue::Null)
        }
    } else if value.is_string() {
        let s = value.to_string(scope).unwrap().to_rust_string_lossy(scope);
        JsonValue::String(s)
    } else if value.is_array() {
        let array = v8::Local::<v8::Array>::try_from(value).unwrap();
        let mut vec = Vec::new();
        for i in 0..array.length() {
            if let Some(elem) = array.get_index(scope, i) {
                vec.push(v8_to_json(scope, elem));
            }
        }
        JsonValue::Array(vec)
    } else if value.is_object() {
        let obj = value.to_object(scope).unwrap();
        let mut map = serde_json::Map::new();

        if let Some(prop_names) = obj.get_own_property_names(scope, v8::GetPropertyNamesArgs::default()) {
            for i in 0..prop_names.length() {
                if let Some(key) = prop_names.get_index(scope, i) {
                    let key_str = key.to_string(scope).unwrap().to_rust_string_lossy(scope);
                    if let Some(val) = obj.get(scope, key) {
                        map.insert(key_str, v8_to_json(scope, val));
                    }
                }
            }
        }
        JsonValue::Object(map)
    } else {
        // Fallback: try to convert to string
        value
            .to_string(scope)
            .map(|s| JsonValue::String(s.to_rust_string_lossy(scope)))
            .unwrap_or(JsonValue::Null)
    }
}

/// Convert a JSON value to a V8 value
fn json_to_v8<'s>(scope: &mut v8::HandleScope<'s>, value: &JsonValue) -> v8::Local<'s, v8::Value> {
    match value {
        JsonValue::Null => v8::null(scope).into(),
        JsonValue::Bool(b) => v8::Boolean::new(scope, *b).into(),
        JsonValue::Number(n) => {
            if let Some(i) = n.as_i64() {
                v8::Integer::new(scope, i as i32).into()
            } else if let Some(f) = n.as_f64() {
                v8::Number::new(scope, f).into()
            } else {
                v8::undefined(scope).into()
            }
        }
        JsonValue::String(s) => v8::String::new(scope, s)
            .map(|v| v.into())
            .unwrap_or_else(|| v8::undefined(scope).into()),
        JsonValue::Array(arr) => {
            let v8_array = v8::Array::new(scope, arr.len() as i32);
            for (i, elem) in arr.iter().enumerate() {
                let v8_elem = json_to_v8(scope, elem);
                v8_array.set_index(scope, i as u32, v8_elem);
            }
            v8_array.into()
        }
        JsonValue::Object(map) => {
            let v8_obj = v8::Object::new(scope);
            for (key, val) in map {
                let v8_key = v8::String::new(scope, key).unwrap();
                let v8_val = json_to_v8(scope, val);
                v8_obj.set(scope, v8_key.into(), v8_val);
            }
            v8_obj.into()
        }
    }
}

/// Set up custom functions on a V8 context
fn setup_custom_functions(
    scope: &mut v8::HandleScope,
    context: v8::Local<v8::Context>,
    functions: &[CustomFunction],
) {
    let global = context.global(scope);

    for func in functions {
        // Create a string with the function name to use as data
        let func_name = v8::String::new(scope, &func.name).unwrap();

        // Create the function template with the name as external data
        let func_template = v8::FunctionTemplate::builder(custom_function_callback)
            .data(func_name.into())
            .build(scope);

        // Get the function instance
        if let Some(function) = func_template.get_function(scope) {
            // Set it on the global object
            let key = v8::String::new(scope, &func.name).unwrap();
            global.set(scope, key.into(), function.into());
        }
    }
}

/// Execute JavaScript code with custom functions in a stateless isolate
pub fn execute_with_custom_functions(
    code: String,
    functions: &[CustomFunction],
) -> Result<String, String> {
    // Register functions for this thread
    register_functions(functions);

    let result = {
        let isolate = &mut v8::Isolate::new(Default::default());
        let scope = &mut v8::HandleScope::new(isolate);
        let context = v8::Context::new(scope, Default::default());
        let scope = &mut v8::ContextScope::new(scope, context);

        // Set up custom functions
        setup_custom_functions(scope, context, functions);

        // Execute the code
        let result = eval(scope, &code)?;
        match result.to_string(scope) {
            Some(s) => Ok(s.to_rust_string_lossy(scope)),
            None => Err("Failed to convert result to string".to_string()),
        }
    };

    // Clean up
    clear_functions();

    result
}

/// Execute JavaScript code with custom functions and snapshot support
pub fn execute_with_custom_functions_stateful(
    code: String,
    snapshot: Option<Vec<u8>>,
    functions: &[CustomFunction],
) -> Result<(String, Vec<u8>), String> {
    // Register functions for this thread
    register_functions(functions);

    let result = {
        let mut snapshot_creator = match snapshot {
            Some(snapshot) => {
                v8::Isolate::snapshot_creator_from_existing_snapshot(snapshot, None, None)
            }
            None => {
                v8::Isolate::snapshot_creator(Default::default(), Default::default())
            }
        };

        let mut output_result: Result<String, String> = Err("Unknown error".to_string());
        {
            let scope = &mut v8::HandleScope::new(&mut snapshot_creator);
            let context = v8::Context::new(scope, Default::default());
            let scope = &mut v8::ContextScope::new(scope, context);

            // Set up custom functions
            setup_custom_functions(scope, context, functions);

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
    };

    // Clean up
    clear_functions();

    result
}

/// Helper function to evaluate JavaScript code (same as in mcp.rs)
fn eval<'s>(scope: &mut v8::HandleScope<'s>, code: &str) -> Result<v8::Local<'s, v8::Value>, String> {
    let scope = &mut v8::EscapableHandleScope::new(scope);
    let source = v8::String::new(scope, code).ok_or("Failed to create V8 string")?;
    let script = v8::Script::compile(scope, source, None).ok_or("Failed to compile script")?;
    let r = script.run(scope).ok_or("Failed to run script")?;
    Ok(scope.escape(r))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::initialize_v8;

    fn setup() {
        initialize_v8();
    }

    #[test]
    fn test_simple_custom_function() {
        setup();

        fn double(args: &[JsonValue]) -> Result<JsonValue, String> {
            let n = args.get(0).and_then(|v| v.as_f64()).unwrap_or(0.0);
            Ok(serde_json::json!(n * 2.0))
        }

        let functions = vec![CustomFunction::new("double", double)];
        let result = execute_with_custom_functions("double(21)".to_string(), &functions);
        assert_eq!(result.unwrap(), "42");
    }

    #[test]
    fn test_string_function() {
        setup();

        fn greet(args: &[JsonValue]) -> Result<JsonValue, String> {
            let name = args.get(0).and_then(|v| v.as_str()).unwrap_or("World");
            Ok(serde_json::json!(format!("Hello, {}!", name)))
        }

        let functions = vec![CustomFunction::new("greet", greet)];
        let result = execute_with_custom_functions("greet('Rust')".to_string(), &functions);
        assert_eq!(result.unwrap(), "Hello, Rust!");
    }

    #[test]
    fn test_multiple_functions() {
        setup();

        fn add(args: &[JsonValue]) -> Result<JsonValue, String> {
            let a = args.get(0).and_then(|v| v.as_f64()).unwrap_or(0.0);
            let b = args.get(1).and_then(|v| v.as_f64()).unwrap_or(0.0);
            Ok(serde_json::json!(a + b))
        }

        fn multiply(args: &[JsonValue]) -> Result<JsonValue, String> {
            let a = args.get(0).and_then(|v| v.as_f64()).unwrap_or(0.0);
            let b = args.get(1).and_then(|v| v.as_f64()).unwrap_or(0.0);
            Ok(serde_json::json!(a * b))
        }

        let functions = vec![
            CustomFunction::new("add", add),
            CustomFunction::new("multiply", multiply),
        ];

        let result = execute_with_custom_functions("add(multiply(2, 3), 4)".to_string(), &functions);
        assert_eq!(result.unwrap(), "10");
    }

    #[test]
    fn test_function_with_array_arg() {
        setup();

        fn sum_array(args: &[JsonValue]) -> Result<JsonValue, String> {
            let arr = args.get(0).and_then(|v| v.as_array()).ok_or("Expected array")?;
            let sum: f64 = arr.iter().filter_map(|v| v.as_f64()).sum();
            Ok(serde_json::json!(sum))
        }

        let functions = vec![CustomFunction::new("sumArray", sum_array)];
        let result = execute_with_custom_functions("sumArray([1, 2, 3, 4, 5])".to_string(), &functions);
        assert_eq!(result.unwrap(), "15");
    }

    #[test]
    fn test_function_returning_object() {
        setup();

        fn create_person(args: &[JsonValue]) -> Result<JsonValue, String> {
            let name = args.get(0).and_then(|v| v.as_str()).unwrap_or("Unknown");
            let age = args.get(1).and_then(|v| v.as_i64()).unwrap_or(0);
            Ok(serde_json::json!({
                "name": name,
                "age": age
            }))
        }

        let functions = vec![CustomFunction::new("createPerson", create_person)];
        let result = execute_with_custom_functions(
            "JSON.stringify(createPerson('Alice', 30))".to_string(),
            &functions
        );
        let result_str = result.unwrap();
        assert!(result_str.contains("Alice"));
        assert!(result_str.contains("30"));
    }

    #[test]
    fn test_function_error() {
        setup();

        fn fail_func(_args: &[JsonValue]) -> Result<JsonValue, String> {
            Err("This function always fails".to_string())
        }

        let functions = vec![CustomFunction::new("failFunc", fail_func)];
        let result = execute_with_custom_functions("failFunc()".to_string(), &functions);
        assert!(result.is_err());
    }
}
