//! Static-library packaging for the canonical UniFFI API defined by `server`.

pub use server::library::{
    LibraryConfig, LibraryError, LibraryMode, McpJsLibrary, ToolDefinition,
    default_library_config,
};
