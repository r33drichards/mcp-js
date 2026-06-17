//! mcp-v8 API Client
//!
//! This crate provides an auto-generated Rust client for the mcp-v8 HTTP API.
//! The client is generated at build time from `openapi.json` using
//! [progenitor](https://crates.io/crates/progenitor).
//!
//! ## Quick start
//!
//! ```no_run
//! use mcp_v8_client::Client;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let client = Client::new("http://localhost:3000");
//!
//!     // Submit JavaScript code for async execution
//!     let body = mcp_v8_client::types::ExecRequest {
//!         code: "1 + 1".to_string(),
//!         heap: None,
//!         session: None,
//!         heap_memory_max_mb: None,
//!         execution_timeout_secs: None,
//!         tags: None,
//!     };
//!     let resp = client.exec_handler(&body).await?;
//!     println!("execution_id: {}", resp.execution_id);
//!
//!     Ok(())
//! }
//! ```




include!(concat!(env!("OUT_DIR"), "/codegen.rs"));
