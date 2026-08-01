//! Static-library packaging for the canonical UniFFI API defined by `server`.

pub use server::engine::execution::{ConsoleOutputPage, ExecutionInfo, ExecutionSummary};
pub use server::engine::fs_merge::Prefer;
pub use server::engine::heap_tags::HeapTagEntry;
pub use server::engine::session_log::SessionSnapshotView;
pub use server::engine::{
    FsLabelView, FsMergeConflictView, FsMergeResult, FsPushOutcome, FsRefLogView,
};
pub use server::engine::{
    DEFAULT_EXECUTION_TIMEOUT_SECS, DEFAULT_MCP_STUB_PREFIX, DEFAULT_WASM_STUB_PREFIX,
    RuntimeCapabilities, RuntimeCapabilityConfig, RuntimeOptions, RuntimeError,
    ExecutionRequest, RuntimeFeatureConfig, RuntimeFetchHeaderRule,
    RuntimeFetchOAuthConfig, RuntimeHardeningConfig, RuntimeLifecycleState,
    McpRequestHeaders, RuntimeMcpServerConfig, RuntimeMcpStubConfig,
    RuntimeMcpTransportKind, RuntimeMode, RuntimeOperationPolicies, RuntimePolicyConfig,
    RuntimePolicyEvalMode, RuntimePolicySource, RuntimeRunJsFileAccess, RuntimeConfig,
    RuntimeShutdownResult, RuntimeStorageKind, ToolCallRequest, RuntimeUpstreamMcpConfig,
    RuntimeWasmModuleConfig, RuntimeWasmStubConfig, McpJsRuntime, ToolDefinition, create_runtime,
    create_runtime_with_configuration, create_runtime_with_features, create_runtime_with_upstreams,
    default_capability_config, default_feature_config, default_fetch_oauth_refresh_buffer_secs,
    default_runtime_options, default_policy_config, default_runtime_config,
    default_upstream_mcp_config,
};
