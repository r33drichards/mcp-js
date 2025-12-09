//! Integration tests for custom Rust functions in JavaScript
//!
//! These tests verify that custom Rust functions can be properly
//! registered and called from JavaScript code in the V8 isolate.

use server::{
    initialize_v8,
    custom_functions::{CustomFunction, execute_with_custom_functions, execute_with_custom_functions_stateful},
};
use serde_json::{json, Value as JsonValue};
use std::sync::Once;

static INIT: Once = Once::new();

fn setup() {
    INIT.call_once(|| {
        initialize_v8();
    });
}

// =============================================================================
// Basic Function Tests
// =============================================================================

#[test]
fn test_simple_addition_function() {
    setup();

    fn add(args: &[JsonValue]) -> Result<JsonValue, String> {
        let a = args.get(0).and_then(|v| v.as_f64()).unwrap_or(0.0);
        let b = args.get(1).and_then(|v| v.as_f64()).unwrap_or(0.0);
        Ok(json!(a + b))
    }

    let functions = vec![CustomFunction::new("add", add)];
    let result = execute_with_custom_functions("add(2, 3)".to_string(), &functions);

    assert_eq!(result.unwrap(), "5");
}

#[test]
fn test_multiplication_function() {
    setup();

    fn multiply(args: &[JsonValue]) -> Result<JsonValue, String> {
        let a = args.get(0).and_then(|v| v.as_f64()).unwrap_or(0.0);
        let b = args.get(1).and_then(|v| v.as_f64()).unwrap_or(0.0);
        Ok(json!(a * b))
    }

    let functions = vec![CustomFunction::new("multiply", multiply)];
    let result = execute_with_custom_functions("multiply(6, 7)".to_string(), &functions);

    assert_eq!(result.unwrap(), "42");
}

#[test]
fn test_function_with_no_arguments() {
    setup();

    fn get_magic_number(_args: &[JsonValue]) -> Result<JsonValue, String> {
        Ok(json!(42))
    }

    let functions = vec![CustomFunction::new("getMagicNumber", get_magic_number)];
    let result = execute_with_custom_functions("getMagicNumber()".to_string(), &functions);

    assert_eq!(result.unwrap(), "42");
}

#[test]
fn test_function_with_default_arguments() {
    setup();

    fn greet(args: &[JsonValue]) -> Result<JsonValue, String> {
        let name = args.get(0).and_then(|v| v.as_str()).unwrap_or("World");
        Ok(json!(format!("Hello, {}!", name)))
    }

    let functions = vec![CustomFunction::new("greet", greet)];

    // With argument
    let result = execute_with_custom_functions("greet('Rust')".to_string(), &functions);
    assert_eq!(result.unwrap(), "Hello, Rust!");

    // Without argument (uses default)
    let result = execute_with_custom_functions("greet()".to_string(), &functions);
    assert_eq!(result.unwrap(), "Hello, World!");
}

// =============================================================================
// Multiple Functions Tests
// =============================================================================

#[test]
fn test_multiple_functions_registered() {
    setup();

    fn add(args: &[JsonValue]) -> Result<JsonValue, String> {
        let a = args.get(0).and_then(|v| v.as_f64()).unwrap_or(0.0);
        let b = args.get(1).and_then(|v| v.as_f64()).unwrap_or(0.0);
        Ok(json!(a + b))
    }

    fn subtract(args: &[JsonValue]) -> Result<JsonValue, String> {
        let a = args.get(0).and_then(|v| v.as_f64()).unwrap_or(0.0);
        let b = args.get(1).and_then(|v| v.as_f64()).unwrap_or(0.0);
        Ok(json!(a - b))
    }

    fn multiply(args: &[JsonValue]) -> Result<JsonValue, String> {
        let a = args.get(0).and_then(|v| v.as_f64()).unwrap_or(0.0);
        let b = args.get(1).and_then(|v| v.as_f64()).unwrap_or(0.0);
        Ok(json!(a * b))
    }

    let functions = vec![
        CustomFunction::new("add", add),
        CustomFunction::new("subtract", subtract),
        CustomFunction::new("multiply", multiply),
    ];

    assert_eq!(execute_with_custom_functions("add(5, 3)".to_string(), &functions).unwrap(), "8");
    assert_eq!(execute_with_custom_functions("subtract(10, 4)".to_string(), &functions).unwrap(), "6");
    assert_eq!(execute_with_custom_functions("multiply(7, 6)".to_string(), &functions).unwrap(), "42");
}

#[test]
fn test_composing_custom_functions() {
    setup();

    fn add(args: &[JsonValue]) -> Result<JsonValue, String> {
        let a = args.get(0).and_then(|v| v.as_f64()).unwrap_or(0.0);
        let b = args.get(1).and_then(|v| v.as_f64()).unwrap_or(0.0);
        Ok(json!(a + b))
    }

    fn multiply(args: &[JsonValue]) -> Result<JsonValue, String> {
        let a = args.get(0).and_then(|v| v.as_f64()).unwrap_or(0.0);
        let b = args.get(1).and_then(|v| v.as_f64()).unwrap_or(0.0);
        Ok(json!(a * b))
    }

    let functions = vec![
        CustomFunction::new("add", add),
        CustomFunction::new("multiply", multiply),
    ];

    // (3 * 4) + 5 = 17
    let result = execute_with_custom_functions("add(multiply(3, 4), 5)".to_string(), &functions);
    assert_eq!(result.unwrap(), "17");

    // (2 + 3) * (4 + 5) = 5 * 9 = 45
    let result = execute_with_custom_functions("multiply(add(2, 3), add(4, 5))".to_string(), &functions);
    assert_eq!(result.unwrap(), "45");
}

// =============================================================================
// Array Argument Tests
// =============================================================================

#[test]
fn test_function_with_array_argument() {
    setup();

    fn sum_array(args: &[JsonValue]) -> Result<JsonValue, String> {
        let arr = args.get(0).and_then(|v| v.as_array()).ok_or("Expected array")?;
        let sum: f64 = arr.iter().filter_map(|v| v.as_f64()).sum();
        Ok(json!(sum))
    }

    let functions = vec![CustomFunction::new("sumArray", sum_array)];

    let result = execute_with_custom_functions("sumArray([1, 2, 3, 4, 5])".to_string(), &functions);
    assert_eq!(result.unwrap(), "15");
}

#[test]
fn test_function_with_array_of_strings() {
    setup();

    fn join_strings(args: &[JsonValue]) -> Result<JsonValue, String> {
        let arr = args.get(0).and_then(|v| v.as_array()).ok_or("Expected array")?;
        let separator = args.get(1).and_then(|v| v.as_str()).unwrap_or(", ");
        let strings: Vec<&str> = arr.iter().filter_map(|v| v.as_str()).collect();
        Ok(json!(strings.join(separator)))
    }

    let functions = vec![CustomFunction::new("joinStrings", join_strings)];

    let result = execute_with_custom_functions(
        r#"joinStrings(['hello', 'world'], ' ')"#.to_string(),
        &functions
    );
    assert_eq!(result.unwrap(), "hello world");
}

#[test]
fn test_function_returning_array() {
    setup();

    fn reverse_array(args: &[JsonValue]) -> Result<JsonValue, String> {
        let arr = args.get(0).and_then(|v| v.as_array()).ok_or("Expected array")?;
        let reversed: Vec<JsonValue> = arr.iter().rev().cloned().collect();
        Ok(json!(reversed))
    }

    let functions = vec![CustomFunction::new("reverseArray", reverse_array)];

    let result = execute_with_custom_functions(
        "JSON.stringify(reverseArray([1, 2, 3]))".to_string(),
        &functions
    );
    assert_eq!(result.unwrap(), "[3,2,1]");
}

// =============================================================================
// Object Argument Tests
// =============================================================================

#[test]
fn test_function_with_object_argument() {
    setup();

    fn get_name(args: &[JsonValue]) -> Result<JsonValue, String> {
        let obj = args.get(0).and_then(|v| v.as_object()).ok_or("Expected object")?;
        let name = obj.get("name").and_then(|v| v.as_str()).unwrap_or("Unknown");
        Ok(json!(name))
    }

    let functions = vec![CustomFunction::new("getName", get_name)];

    let result = execute_with_custom_functions(
        r#"getName({name: 'Alice', age: 30})"#.to_string(),
        &functions
    );
    assert_eq!(result.unwrap(), "Alice");
}

#[test]
fn test_function_returning_object() {
    setup();

    fn create_person(args: &[JsonValue]) -> Result<JsonValue, String> {
        let name = args.get(0).and_then(|v| v.as_str()).unwrap_or("Unknown");
        let age = args.get(1).and_then(|v| v.as_i64()).unwrap_or(0);
        Ok(json!({
            "name": name,
            "age": age,
            "greeting": format!("Hello, I'm {}", name)
        }))
    }

    let functions = vec![CustomFunction::new("createPerson", create_person)];

    let result = execute_with_custom_functions(
        r#"JSON.stringify(createPerson('Bob', 25))"#.to_string(),
        &functions
    );
    let result_str = result.unwrap();
    assert!(result_str.contains("Bob"));
    assert!(result_str.contains("25"));
    assert!(result_str.contains("Hello, I'm Bob"));
}

// =============================================================================
// Error Handling Tests
// =============================================================================

#[test]
fn test_function_returning_error() {
    setup();

    fn divide(args: &[JsonValue]) -> Result<JsonValue, String> {
        let a = args.get(0).and_then(|v| v.as_f64()).ok_or("First argument must be a number")?;
        let b = args.get(1).and_then(|v| v.as_f64()).ok_or("Second argument must be a number")?;

        if b == 0.0 {
            return Err("Division by zero".to_string());
        }

        Ok(json!(a / b))
    }

    let functions = vec![CustomFunction::new("divide", divide)];

    // Successful division
    let result = execute_with_custom_functions("divide(10, 2)".to_string(), &functions);
    assert_eq!(result.unwrap(), "5");

    // Division by zero should fail
    let result = execute_with_custom_functions("divide(10, 0)".to_string(), &functions);
    assert!(result.is_err());
}

#[test]
fn test_function_with_validation() {
    setup();

    fn validate_email(args: &[JsonValue]) -> Result<JsonValue, String> {
        let email = args.get(0).and_then(|v| v.as_str()).ok_or("Email must be a string")?;

        if !email.contains('@') {
            return Err(format!("Invalid email: {}", email));
        }

        Ok(json!(true))
    }

    let functions = vec![CustomFunction::new("validateEmail", validate_email)];

    let result = execute_with_custom_functions(
        r#"validateEmail('test@example.com')"#.to_string(),
        &functions
    );
    assert_eq!(result.unwrap(), "true");

    let result = execute_with_custom_functions(
        r#"validateEmail('invalid-email')"#.to_string(),
        &functions
    );
    assert!(result.is_err());
}

// =============================================================================
// Integration with JavaScript Tests
// =============================================================================

#[test]
fn test_custom_function_in_js_expression() {
    setup();

    fn double(args: &[JsonValue]) -> Result<JsonValue, String> {
        let n = args.get(0).and_then(|v| v.as_f64()).unwrap_or(0.0);
        Ok(json!(n * 2.0))
    }

    let functions = vec![CustomFunction::new("double", double)];

    // Use custom function in array map
    let result = execute_with_custom_functions(
        "[1, 2, 3].map(x => double(x)).reduce((a, b) => a + b, 0)".to_string(),
        &functions
    );
    assert_eq!(result.unwrap(), "12"); // 2 + 4 + 6 = 12
}

#[test]
fn test_custom_function_with_js_variables() {
    setup();

    fn multiply(args: &[JsonValue]) -> Result<JsonValue, String> {
        let a = args.get(0).and_then(|v| v.as_f64()).unwrap_or(0.0);
        let b = args.get(1).and_then(|v| v.as_f64()).unwrap_or(0.0);
        Ok(json!(a * b))
    }

    let functions = vec![CustomFunction::new("multiply", multiply)];

    let code = r#"
        var x = 5;
        var y = 10;
        multiply(x, y)
    "#;

    let result = execute_with_custom_functions(code.to_string(), &functions);
    assert_eq!(result.unwrap(), "50");
}

#[test]
fn test_custom_function_in_js_function() {
    setup();

    fn square(args: &[JsonValue]) -> Result<JsonValue, String> {
        let n = args.get(0).and_then(|v| v.as_f64()).unwrap_or(0.0);
        Ok(json!(n * n))
    }

    let functions = vec![CustomFunction::new("square", square)];

    let code = r#"
        function sumOfSquares(arr) {
            return arr.reduce((sum, n) => sum + square(n), 0);
        }
        sumOfSquares([1, 2, 3, 4])
    "#;

    let result = execute_with_custom_functions(code.to_string(), &functions);
    assert_eq!(result.unwrap(), "30"); // 1 + 4 + 9 + 16 = 30
}

// =============================================================================
// Stateful Execution Tests
// =============================================================================

#[test]
fn test_custom_functions_with_stateful_execution() {
    setup();

    fn increment(args: &[JsonValue]) -> Result<JsonValue, String> {
        let n = args.get(0).and_then(|v| v.as_f64()).unwrap_or(0.0);
        Ok(json!(n + 1.0))
    }

    let functions = vec![CustomFunction::new("increment", increment)];

    // First execution: set up a variable
    let (result1, snapshot) = execute_with_custom_functions_stateful(
        "var counter = increment(0); counter".to_string(),
        None,
        &functions,
    ).unwrap();
    assert_eq!(result1, "1");

    // Second execution: use the persisted variable with custom function
    let (result2, snapshot) = execute_with_custom_functions_stateful(
        "counter = increment(counter); counter".to_string(),
        Some(snapshot),
        &functions,
    ).unwrap();
    assert_eq!(result2, "2");

    // Third execution: continue using persisted state
    let (result3, _) = execute_with_custom_functions_stateful(
        "counter = increment(counter); counter".to_string(),
        Some(snapshot),
        &functions,
    ).unwrap();
    assert_eq!(result3, "3");
}

#[test]
fn test_stateful_with_multiple_custom_functions() {
    setup();

    fn add(args: &[JsonValue]) -> Result<JsonValue, String> {
        let a = args.get(0).and_then(|v| v.as_f64()).unwrap_or(0.0);
        let b = args.get(1).and_then(|v| v.as_f64()).unwrap_or(0.0);
        Ok(json!(a + b))
    }

    fn multiply(args: &[JsonValue]) -> Result<JsonValue, String> {
        let a = args.get(0).and_then(|v| v.as_f64()).unwrap_or(0.0);
        let b = args.get(1).and_then(|v| v.as_f64()).unwrap_or(0.0);
        Ok(json!(a * b))
    }

    let functions = vec![
        CustomFunction::new("add", add),
        CustomFunction::new("multiply", multiply),
    ];

    // First: set base value using multiply
    let (result, snapshot) = execute_with_custom_functions_stateful(
        "var base = multiply(10, 10); base".to_string(),
        None,
        &functions,
    ).unwrap();
    assert_eq!(result, "100");

    // Second: use add with persisted base
    let (result, _) = execute_with_custom_functions_stateful(
        "add(base, 23)".to_string(),
        Some(snapshot),
        &functions,
    ).unwrap();
    assert_eq!(result, "123");
}

// =============================================================================
// Edge Case Tests
// =============================================================================

#[test]
fn test_empty_function_list() {
    setup();

    let functions: Vec<CustomFunction> = vec![];
    let result = execute_with_custom_functions("1 + 1".to_string(), &functions);
    assert_eq!(result.unwrap(), "2");
}

#[test]
fn test_function_with_many_arguments() {
    setup();

    fn sum_all(args: &[JsonValue]) -> Result<JsonValue, String> {
        let sum: f64 = args.iter().filter_map(|v| v.as_f64()).sum();
        Ok(json!(sum))
    }

    let functions = vec![CustomFunction::new("sumAll", sum_all)];

    let result = execute_with_custom_functions(
        "sumAll(1, 2, 3, 4, 5, 6, 7, 8, 9, 10)".to_string(),
        &functions
    );
    assert_eq!(result.unwrap(), "55");
}

#[test]
fn test_function_with_nested_objects() {
    setup();

    fn get_nested_value(args: &[JsonValue]) -> Result<JsonValue, String> {
        let obj = args.get(0).ok_or("Expected object")?;
        let path: Vec<&str> = args.get(1)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .split('.')
            .collect();

        let mut current = obj;
        for key in path {
            current = current.get(key).ok_or(format!("Key '{}' not found", key))?;
        }

        Ok(current.clone())
    }

    let functions = vec![CustomFunction::new("getNestedValue", get_nested_value)];

    let code = r#"
        var data = {
            user: {
                profile: {
                    name: 'Alice'
                }
            }
        };
        getNestedValue(data, 'user.profile.name')
    "#;

    let result = execute_with_custom_functions(code.to_string(), &functions);
    assert_eq!(result.unwrap(), "Alice");
}

#[test]
fn test_function_returning_null() {
    setup();

    fn return_null(_args: &[JsonValue]) -> Result<JsonValue, String> {
        Ok(JsonValue::Null)
    }

    let functions = vec![CustomFunction::new("returnNull", return_null)];

    let result = execute_with_custom_functions("returnNull()".to_string(), &functions);
    assert_eq!(result.unwrap(), "null");
}

#[test]
fn test_function_returning_boolean() {
    setup();

    fn is_even(args: &[JsonValue]) -> Result<JsonValue, String> {
        let n = args.get(0).and_then(|v| v.as_i64()).unwrap_or(0);
        Ok(json!(n % 2 == 0))
    }

    let functions = vec![CustomFunction::new("isEven", is_even)];

    assert_eq!(
        execute_with_custom_functions("isEven(4)".to_string(), &functions).unwrap(),
        "true"
    );
    assert_eq!(
        execute_with_custom_functions("isEven(7)".to_string(), &functions).unwrap(),
        "false"
    );
}
