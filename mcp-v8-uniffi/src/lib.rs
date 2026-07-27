//! Static-library packaging for the canonical UniFFI API defined by `server`.

pub use server::library::{
    LibraryCapabilities, LibraryCapabilityConfig, LibraryConfig, LibraryError,
    LibraryExecutionInfo, LibraryExecutionOutput, LibraryExecutionRequest, LibraryExecutionSummary,
    LibraryFeatureConfig, LibraryFetchHeaderRule, LibraryFetchOAuthConfig, LibraryFsLabel,
    LibraryFsMergeConflict, LibraryFsMergePreference, LibraryFsMergeResult, LibraryFsPushResult,
    LibraryFsRefLogEntry, LibraryHardeningConfig, LibraryHeapTagEntry, LibraryLifecycleState,
    LibraryMode, LibraryOperationPolicies, LibraryPolicyConfig, LibraryPolicyEvalMode,
    LibraryPolicySource, LibraryRunJsFileAccess, LibraryRuntimeConfig, LibraryShutdownResult,
    LibraryStorageKind, LibraryToolCallRequest, LibraryWasmModuleConfig, LibraryWasmStubConfig,
    McpJsLibrary, ToolDefinition, create_library, create_library_with_configuration,
    create_library_with_features, default_capability_config, default_feature_config,
    default_library_config, default_policy_config, default_runtime_config,
};
