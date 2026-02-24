/// Tests for TypeScript support — verifies that TypeScript code is correctly
/// stripped of type annotations and executed by V8.
///
/// Uses Engine::run_js (the production async path) to test the full pipeline:
/// SWC type stripping → V8 execution.

use std::sync::Once;
use server::engine::{initialize_v8, strip_typescript_types, Engine};

static INIT: Once = Once::new();

fn ensure_v8() {
    INIT.call_once(|| {
        initialize_v8();
    });
}

// ── strip_typescript_types unit tests ────────────────────────────────────

#[test]
fn test_strip_plain_javascript_passthrough() {
    let js = "const x = 1 + 2; x;";
    let result = strip_typescript_types(js).unwrap();
    assert!(result.contains("1 + 2"), "Plain JS should pass through, got: {}", result);
}

#[test]
fn test_strip_type_annotation() {
    let ts = "const x: number = 42; x;";
    let result = strip_typescript_types(ts).unwrap();
    assert!(!result.contains(": number"), "Type annotation should be stripped, got: {}", result);
    assert!(result.contains("42"), "Value should remain, got: {}", result);
}

#[test]
fn test_strip_interface() {
    let ts = r#"interface Foo { bar: string; } const f: Foo = { bar: "hello" }; JSON.stringify(f);"#;
    let result = strip_typescript_types(ts).unwrap();
    assert!(!result.contains("interface"), "Interface should be stripped, got: {}", result);
    assert!(!result.contains(": Foo"), "Type reference should be stripped, got: {}", result);
    assert!(result.contains("bar"), "Property name should remain, got: {}", result);
}

#[test]
fn test_strip_function_parameter_types() {
    let ts = "function add(a: number, b: number): number { return a + b; }";
    let result = strip_typescript_types(ts).unwrap();
    assert!(!result.contains(": number"), "Parameter types should be stripped, got: {}", result);
    assert!(result.contains("return a + b"), "Function body should remain, got: {}", result);
}

#[test]
fn test_strip_generics() {
    let ts = "function identity<T>(x: T): T { return x; }";
    let result = strip_typescript_types(ts).unwrap();
    assert!(!result.contains("<T>"), "Generic should be stripped, got: {}", result);
    assert!(result.contains("return x"), "Body should remain, got: {}", result);
}

#[test]
fn test_strip_type_alias() {
    let ts = "type ID = string | number; const id: ID = 'abc'; id;";
    let result = strip_typescript_types(ts).unwrap();
    assert!(!result.contains("type ID"), "Type alias should be stripped, got: {}", result);
}

#[test]
fn test_strip_as_expression() {
    let ts = "const x = (42 as number); x;";
    let result = strip_typescript_types(ts).unwrap();
    assert!(!result.contains("as number"), "'as' cast should be stripped, got: {}", result);
    assert!(result.contains("42"), "Value should remain, got: {}", result);
}

#[test]
fn test_strip_empty_input() {
    let result = strip_typescript_types("").unwrap();
    assert!(result.trim().is_empty(), "Empty input should produce empty output, got: {:?}", result);
}

#[test]
fn test_strip_parse_error() {
    let bad = "function(";
    let result = strip_typescript_types(bad);
    assert!(result.is_err(), "Invalid syntax should return an error");
}

// ── Full pipeline tests (SWC strip → V8 execution) ──────────────────────

#[tokio::test]
async fn test_typescript_basic_types() {
    ensure_v8();
    let engine = Engine::new_stateless(8 * 1024 * 1024, 30, 4);

    let result = engine.run_js(
        "let x: number = 42; x;".to_string(),
        None, None, None, None,
    ).await;

    assert!(result.is_ok(), "Typed variable should execute, got: {:?}", result);
    assert_eq!(result.unwrap().output, "42");
}

#[tokio::test]
async fn test_typescript_function_with_types() {
    ensure_v8();
    let engine = Engine::new_stateless(8 * 1024 * 1024, 30, 4);

    let result = engine.run_js(
        "function add(a: number, b: number): number { return a + b; } add(3, 4);".to_string(),
        None, None, None, None,
    ).await;

    assert!(result.is_ok(), "Typed function should execute, got: {:?}", result);
    assert_eq!(result.unwrap().output, "7");
}

#[tokio::test]
async fn test_typescript_arrow_function_with_types() {
    ensure_v8();
    let engine = Engine::new_stateless(8 * 1024 * 1024, 30, 4);

    let result = engine.run_js(
        "const multiply = (a: number, b: number): number => a * b; multiply(6, 7);".to_string(),
        None, None, None, None,
    ).await;

    assert!(result.is_ok(), "Typed arrow function should execute, got: {:?}", result);
    assert_eq!(result.unwrap().output, "42");
}

#[tokio::test]
async fn test_typescript_interface_and_object() {
    ensure_v8();
    let engine = Engine::new_stateless(8 * 1024 * 1024, 30, 4);

    let ts = r#"
        interface Person {
            name: string;
            age: number;
        }
        const p: Person = { name: "Alice", age: 30 };
        JSON.stringify(p);
    "#;

    let result = engine.run_js(ts.to_string(), None, None, None, None).await;

    assert!(result.is_ok(), "Interface + typed object should execute, got: {:?}", result);
    assert_eq!(result.unwrap().output, r#"{"name":"Alice","age":30}"#);
}

#[tokio::test]
async fn test_typescript_generics_execution() {
    ensure_v8();
    let engine = Engine::new_stateless(8 * 1024 * 1024, 30, 4);

    let ts = r#"
        function identity<T>(x: T): T { return x; }
        identity<number>(99);
    "#;

    let result = engine.run_js(ts.to_string(), None, None, None, None).await;

    assert!(result.is_ok(), "Generic function should execute, got: {:?}", result);
    assert_eq!(result.unwrap().output, "99");
}

#[tokio::test]
async fn test_typescript_type_alias_execution() {
    ensure_v8();
    let engine = Engine::new_stateless(8 * 1024 * 1024, 30, 4);

    let ts = r#"
        type StringOrNumber = string | number;
        const val: StringOrNumber = "hello";
        val;
    "#;

    let result = engine.run_js(ts.to_string(), None, None, None, None).await;

    assert!(result.is_ok(), "Type alias should execute, got: {:?}", result);
    assert_eq!(result.unwrap().output, "hello");
}

#[tokio::test]
async fn test_typescript_enum_execution() {
    ensure_v8();
    let engine = Engine::new_stateless(8 * 1024 * 1024, 30, 4);

    let ts = r#"
        enum Direction {
            Up,
            Down,
            Left,
            Right,
        }
        Direction.Down;
    "#;

    let result = engine.run_js(ts.to_string(), None, None, None, None).await;

    assert!(result.is_ok(), "Enum should execute, got: {:?}", result);
    assert_eq!(result.unwrap().output, "1");
}

#[tokio::test]
async fn test_typescript_class_with_types() {
    ensure_v8();
    let engine = Engine::new_stateless(8 * 1024 * 1024, 30, 4);

    let ts = r#"
        class Calculator {
            private value: number;

            constructor(initial: number) {
                this.value = initial;
            }

            add(n: number): Calculator {
                this.value += n;
                return this;
            }

            result(): number {
                return this.value;
            }
        }

        new Calculator(10).add(5).add(3).result();
    "#;

    let result = engine.run_js(ts.to_string(), None, None, None, None).await;

    assert!(result.is_ok(), "Typed class should execute, got: {:?}", result);
    assert_eq!(result.unwrap().output, "18");
}

#[tokio::test]
async fn test_typescript_as_cast() {
    ensure_v8();
    let engine = Engine::new_stateless(8 * 1024 * 1024, 30, 4);

    let result = engine.run_js(
        "const x = (42 as number); x;".to_string(),
        None, None, None, None,
    ).await;

    assert!(result.is_ok(), "'as' cast should execute, got: {:?}", result);
    assert_eq!(result.unwrap().output, "42");
}

#[tokio::test]
async fn test_plain_javascript_still_works() {
    ensure_v8();
    let engine = Engine::new_stateless(8 * 1024 * 1024, 30, 4);

    let result = engine.run_js(
        "var sum = 0; for (var i = 0; i < 10; i++) { sum += i; } sum;".to_string(),
        None, None, None, None,
    ).await;

    assert!(result.is_ok(), "Plain JS should still work, got: {:?}", result);
    assert_eq!(result.unwrap().output, "45");
}
