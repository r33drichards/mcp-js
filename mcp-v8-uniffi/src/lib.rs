//! Static-library packaging for the canonical UniFFI API defined by `server`.

pub use server::library::{
    LibraryCapabilities, LibraryConfig, LibraryError, LibraryExecutionInfo, LibraryExecutionOutput,
    LibraryExecutionSummary, LibraryMode, McpJsLibrary, ToolDefinition, default_library_config,
};
