//! Example: Custom Rust Functions in JavaScript
//!
//! This example demonstrates how to define custom Rust functions
//! that can be called from JavaScript code in the V8 isolate.
//!
//! Run with: `cargo run --example custom_functions`

use server::{
    initialize_v8,
    custom_functions::{CustomFunction, execute_with_custom_functions, execute_with_custom_functions_stateful},
};
use serde_json::{json, Value as JsonValue};

/// A simple function that adds two numbers
fn add(args: &[JsonValue]) -> Result<JsonValue, String> {
    let a = args.get(0).and_then(|v| v.as_f64()).unwrap_or(0.0);
    let b = args.get(1).and_then(|v| v.as_f64()).unwrap_or(0.0);
    Ok(json!(a + b))
}

/// A function that multiplies two numbers
fn multiply(args: &[JsonValue]) -> Result<JsonValue, String> {
    let a = args.get(0).and_then(|v| v.as_f64()).unwrap_or(0.0);
    let b = args.get(1).and_then(|v| v.as_f64()).unwrap_or(0.0);
    Ok(json!(a * b))
}

/// A function that creates a greeting message
fn greet(args: &[JsonValue]) -> Result<JsonValue, String> {
    let name = args.get(0).and_then(|v| v.as_str()).unwrap_or("World");
    Ok(json!(format!("Hello, {}!", name)))
}

/// A function that calculates the sum of an array
fn sum_array(args: &[JsonValue]) -> Result<JsonValue, String> {
    let arr = args.get(0).and_then(|v| v.as_array()).ok_or("Expected array")?;
    let sum: f64 = arr.iter().filter_map(|v| v.as_f64()).sum();
    Ok(json!(sum))
}

/// A function that returns an object with metadata
fn get_metadata(args: &[JsonValue]) -> Result<JsonValue, String> {
    let key = args.get(0).and_then(|v| v.as_str()).unwrap_or("default");
    Ok(json!({
        "key": key,
        "version": "1.0.0",
        "timestamp": 1234567890,
        "features": ["custom_functions", "v8", "rust"]
    }))
}

/// A function that demonstrates error handling
fn safe_divide(args: &[JsonValue]) -> Result<JsonValue, String> {
    let a = args.get(0).and_then(|v| v.as_f64()).ok_or("First argument must be a number")?;
    let b = args.get(1).and_then(|v| v.as_f64()).ok_or("Second argument must be a number")?;

    if b == 0.0 {
        return Err("Division by zero".to_string());
    }

    Ok(json!(a / b))
}

/// A function that transforms strings
fn to_uppercase(args: &[JsonValue]) -> Result<JsonValue, String> {
    let s = args.get(0).and_then(|v| v.as_str()).unwrap_or("");
    Ok(json!(s.to_uppercase()))
}

/// A function that reverses an array
fn reverse_array(args: &[JsonValue]) -> Result<JsonValue, String> {
    let arr = args.get(0).and_then(|v| v.as_array()).ok_or("Expected array")?;
    let reversed: Vec<JsonValue> = arr.iter().rev().cloned().collect();
    Ok(json!(reversed))
}

fn main() {
    // Initialize V8 (must be called once before any JS execution)
    initialize_v8();

    println!("=== Custom Functions in mcp-js ===\n");

    // Define all custom functions
    let functions = vec![
        CustomFunction::new("add", add),
        CustomFunction::new("multiply", multiply),
        CustomFunction::new("greet", greet),
        CustomFunction::new("sumArray", sum_array),
        CustomFunction::new("getMetadata", get_metadata),
        CustomFunction::new("safeDivide", safe_divide),
        CustomFunction::new("toUppercase", to_uppercase),
        CustomFunction::new("reverseArray", reverse_array),
    ];

    // Example 1: Basic arithmetic
    println!("1. Basic arithmetic:");
    let result = execute_with_custom_functions("add(10, 32)".to_string(), &functions);
    println!("   add(10, 32) = {}", result.unwrap());

    let result = execute_with_custom_functions("multiply(6, 7)".to_string(), &functions);
    println!("   multiply(6, 7) = {}", result.unwrap());

    // Example 2: Composing functions
    println!("\n2. Composing functions:");
    let result = execute_with_custom_functions("add(multiply(3, 4), 5)".to_string(), &functions);
    println!("   add(multiply(3, 4), 5) = {}", result.unwrap());

    // Example 3: String functions
    println!("\n3. String functions:");
    let result = execute_with_custom_functions("greet('Rust')".to_string(), &functions);
    println!("   greet('Rust') = {}", result.unwrap());

    let result = execute_with_custom_functions("toUppercase('hello world')".to_string(), &functions);
    println!("   toUppercase('hello world') = {}", result.unwrap());

    // Example 4: Array functions
    println!("\n4. Array functions:");
    let result = execute_with_custom_functions("sumArray([1, 2, 3, 4, 5])".to_string(), &functions);
    println!("   sumArray([1, 2, 3, 4, 5]) = {}", result.unwrap());

    let result = execute_with_custom_functions("JSON.stringify(reverseArray([1, 2, 3]))".to_string(), &functions);
    println!("   reverseArray([1, 2, 3]) = {}", result.unwrap());

    // Example 5: Object return values
    println!("\n5. Object return values:");
    let result = execute_with_custom_functions("JSON.stringify(getMetadata('test'))".to_string(), &functions);
    println!("   getMetadata('test') = {}", result.unwrap());

    // Example 6: Error handling
    println!("\n6. Error handling:");
    let result = execute_with_custom_functions("safeDivide(10, 2)".to_string(), &functions);
    println!("   safeDivide(10, 2) = {}", result.unwrap());

    let result = execute_with_custom_functions("safeDivide(10, 0)".to_string(), &functions);
    println!("   safeDivide(10, 0) = {} (error expected)",
        result.unwrap_or_else(|e| format!("Error: {}", e)));

    // Example 7: Mixing custom functions with JavaScript
    println!("\n7. Mixing with native JavaScript:");
    let code = r#"
        var numbers = [1, 2, 3, 4, 5];
        var doubled = numbers.map(n => multiply(n, 2));
        sumArray(doubled)
    "#;
    let result = execute_with_custom_functions(code.to_string(), &functions);
    println!("   Sum of doubled array: {}", result.unwrap());

    // Example 8: Stateful execution with custom functions
    println!("\n8. Stateful execution with custom functions:");

    // First call: define a variable using custom function result
    let (result, snapshot) = execute_with_custom_functions_stateful(
        "var baseValue = multiply(10, 10); baseValue".to_string(),
        None,
        &functions,
    ).unwrap();
    println!("   First call - baseValue = {}", result);

    // Second call: use the persisted variable with custom function
    let (result, _) = execute_with_custom_functions_stateful(
        "add(baseValue, 23)".to_string(),
        Some(snapshot),
        &functions,
    ).unwrap();
    println!("   Second call - add(baseValue, 23) = {}", result);

    println!("\n=== All examples completed successfully! ===");
}
