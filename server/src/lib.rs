// Library interface for the MCP server
// This allows integration tests to access internal modules

pub mod mcp;

// Re-export initialize_v8 and test helper for integration tests
pub use mcp::initialize_v8;

// Export execute_stateful for integration tests
pub use mcp::execute_stateful_for_test;
