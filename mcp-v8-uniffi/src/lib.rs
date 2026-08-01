//! Static-library packaging for the canonical UniFFI API defined by `server`.

pub use server::engine::execution::{ConsoleOutputPage, ExecutionInfo, ExecutionSummary};
pub use server::engine::fs_merge::Prefer;
pub use server::engine::heap_tags::HeapTagEntry;
pub use server::engine::{
    FsLabelView, FsMergeConflictView, FsMergeResult, FsPushOutcome, FsRefLogView,
};
pub use server::library::{
    DEFAULT_EXECUTION_TIMEOUT_SECS, DEFAULT_MCP_STUB_PREFIX, DEFAULT_WASM_STUB_PREFIX,
    LibraryCapabilities, LibraryCapabilityConfig, LibraryConfig, LibraryError,
    LibraryExecutionRequest, LibraryFeatureConfig, LibraryFetchHeaderRule,
    LibraryFetchOAuthConfig, LibraryHardeningConfig, LibraryLifecycleState,
    LibraryMcpRequestHeaders, LibraryMcpServerConfig, LibraryMcpStubConfig,
    LibraryMcpTransportKind, LibraryMode, LibraryOperationPolicies, LibraryPolicyConfig,
    LibraryPolicyEvalMode, LibraryPolicySource, LibraryRunJsFileAccess, LibraryRuntimeConfig,
    LibraryShutdownResult, LibraryStorageKind, LibraryToolCallRequest, LibraryUpstreamMcpConfig,
    LibraryWasmModuleConfig, LibraryWasmStubConfig, McpJsLibrary, ToolDefinition, create_library,
    create_library_with_configuration, create_library_with_features, create_library_with_upstreams,
    default_capability_config, default_feature_config, default_fetch_oauth_refresh_buffer_secs,
    default_library_config, default_policy_config, default_runtime_config,
    default_upstream_mcp_config,
};
