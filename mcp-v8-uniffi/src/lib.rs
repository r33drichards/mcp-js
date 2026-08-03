//! Static-library packaging for the canonical UniFFI API defined by `server`.

pub use server::engine::execution::{ConsoleOutputPage, ExecutionInfo, ExecutionSummary};
pub use server::engine::fs_merge::Prefer;
pub use server::engine::heap_tags::HeapTagEntry;
pub use server::engine::session_log::SessionSnapshotView;
pub use server::engine::{
    FsLabelView, FsMergeConflictView, FsMergeResult, FsPushOutcome, FsRefLogView,
};
pub use server::engine::{
    DEFAULT_EXECUTION_TIMEOUT_SECS, DEFAULT_MCP_STUB_PREFIX, DEFAULT_WASM_STUB_PREFIX, Engine,
    ExecutionRequest, McpRequestHeaders, RuntimeCapabilities, RuntimeError, RuntimeLifecycleState,
    RuntimeMode, RuntimeShutdownResult, ToolCallRequest, ToolDefinition,
};
