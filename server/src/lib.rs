// Library interface for the MCP server
// This allows integration tests to access internal modules

pub mod mcp;

// Re-export initialize_v8 for integration tests
pub use mcp::initialize_v8;

// Re-export execute_stateful_for_test for integration tests
pub use mcp::execute_stateful_for_test;
