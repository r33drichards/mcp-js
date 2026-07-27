//! Static-library packaging for the canonical UniFFI API defined by `server`.

pub use server::library::{
    LibraryCapabilities, LibraryConfig, LibraryError, LibraryExecutionInfo, LibraryExecutionOutput,
    LibraryExecutionRequest, LibraryExecutionSummary, LibraryFsLabel, LibraryFsMergeConflict,
    LibraryFsMergePreference, LibraryFsMergeResult, LibraryFsPushResult, LibraryFsRefLogEntry,
    LibraryHeapTagEntry, LibraryMode, LibraryRuntimeConfig, LibraryStorageKind, McpJsLibrary,
    ToolDefinition, create_library, default_library_config, default_runtime_config,
};
