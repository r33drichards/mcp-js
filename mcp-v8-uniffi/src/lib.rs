//! Static-library packaging for the canonical UniFFI API defined by `server`.

pub use server::library::{
    LibraryCapabilities, LibraryConfig, LibraryError, LibraryExecutionInfo, LibraryExecutionOutput,
    LibraryExecutionRequest, LibraryExecutionSummary, LibraryFeatureConfig, LibraryFsLabel,
    LibraryFsMergeConflict, LibraryFsMergePreference, LibraryFsMergeResult, LibraryFsPushResult,
    LibraryFsRefLogEntry, LibraryHardeningConfig, LibraryHeapTagEntry, LibraryLifecycleState,
    LibraryMode, LibraryRuntimeConfig, LibraryShutdownResult, LibraryStorageKind,
    LibraryToolCallRequest, LibraryWasmModuleConfig, LibraryWasmStubConfig, McpJsLibrary,
    ToolDefinition, create_library, create_library_with_features, default_feature_config,
    default_library_config, default_runtime_config,
};
