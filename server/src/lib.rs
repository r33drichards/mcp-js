//! MCP JavaScript Execution Server Library
//!
//! This library provides a JavaScript execution environment using V8,
//! with support for custom Rust functions that can be called from JavaScript.
//!
//! # Features
//!
//! - Execute JavaScript code in isolated V8 environments
//! - Stateless execution (fresh environment each time)
//! - Stateful execution with heap persistence via snapshots
//! - Custom Rust functions callable from JavaScript
//!
//! # Example: Custom Functions
//!
//! ```rust,no_run
//! use server::{initialize_v8, custom_functions::{CustomFunction, execute_with_custom_functions}};
//!
//! // Initialize V8 (must be called once before any JS execution)
//! initialize_v8();
//!
//! // Define a custom function
//! fn multiply(args: &[serde_json::Value]) -> Result<serde_json::Value, String> {
//!     let a = args.get(0).and_then(|v| v.as_f64()).unwrap_or(0.0);
//!     let b = args.get(1).and_then(|v| v.as_f64()).unwrap_or(0.0);
//!     Ok(serde_json::json!(a * b))
//! }
//!
//! // Register and use the custom function
//! let functions = vec![CustomFunction::new("multiply", multiply)];
//! let result = execute_with_custom_functions("multiply(6, 7)".to_string(), &functions);
//! assert_eq!(result.unwrap(), "42");
//! ```

pub mod mcp;
pub mod custom_functions;

// Re-export commonly used items
pub use mcp::initialize_v8;
pub use custom_functions::{CustomFunction, CustomFunctionImpl, execute_with_custom_functions, execute_with_custom_functions_stateful};
