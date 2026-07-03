//! Embedding surface for hosting mcp-v8 inside another Rust application.
//!
//! `main.rs` wires the full CLI app (transports, cluster, policies) around the
//! reusable [`Engine`]; this module packages the same wiring as a library API
//! so a host process can run many independent mcp-v8 instances — e.g. one per
//! conversation thread, each with its own fetch header rules (per-thread OAuth
//! tokens) — without spawning subprocesses.
//!
//! Typical use:
//!
//! ```ignore
//! server::embed::initialize(); // once per process
//!
//! let engine = server::embed::build_engine(server::embed::EngineOptions {
//!     fetch: Some(server::embed::FetchOptions::allow_all_with_rules(rules)?),
//!     ..Default::default()
//! })?;
//!
//! // Mount the MCP Streamable HTTP endpoint wherever you like:
//! let app = axum::Router::new().nest("/mcp/thread-42", server::embed::mcp_http_router(engine, None));
//! ```

use std::sync::Arc;

use anyhow::Result;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};

use crate::engine::fetch::{FetchConfig, HeaderRule};
use crate::engine::heap_storage::{AnyHeapStorage, FileHeapStorage};
use crate::engine::heap_tags::HeapTagStore;
use crate::engine::opa::{EvalMode, PolicyChain};
use crate::engine::session_log::SessionLog;
use crate::engine::{self, Engine};
use crate::mcp::{McpService, StatelessMcpService};
use crate::session::SessionVerifier;

pub use crate::fetch_headers::{
    FetchHeaderAuthConfig, FetchHeaderConfigRule, StaticHeadersConfig, rules_from_config,
};

/// One-time process-wide V8 initialization. Must be called exactly once,
/// before the first [`build_engine`] and before spawning Tokio workers that
/// touch V8. Safe to call from any binary that links this crate.
pub fn initialize() {
    engine::initialize_v8();
}

/// Options for building an embedded [`Engine`].
///
/// Defaults mirror a small single-tenant instance: stateless (no heap
/// persistence), no fetch/fs/subprocess capabilities, 512 MB heap cap, 120 s
/// execution timeout, 2 concurrent executions.
pub struct EngineOptions {
    pub heap_memory_max_bytes: usize,
    pub execution_timeout_secs: u64,
    pub max_concurrent_executions: usize,
    /// Enable `fetch()` in the JS runtime.
    pub fetch: Option<FetchOptions>,
    /// Enable heap persistence (stateful sessions) backed by a local directory.
    pub heap: Option<HeapOptions>,
    /// Override the MCP server instructions advertised to clients.
    pub instructions_override: Option<String>,
    /// Override the `run_js` tool description.
    pub run_js_description_override: Option<String>,
}

impl Default for EngineOptions {
    fn default() -> Self {
        Self {
            heap_memory_max_bytes: 512 * 1024 * 1024,
            execution_timeout_secs: 120,
            max_concurrent_executions: 2,
            fetch: None,
            heap: None,
            instructions_override: None,
            run_js_description_override: None,
        }
    }
}

/// Fetch capability configuration for an embedded engine.
pub struct FetchOptions {
    /// Header injection rules (static headers or OAuth client-credentials).
    pub header_rules: Vec<HeaderRule>,
    /// Policy chain gating `fetch()` calls. `None` means allow-all: an empty
    /// [`PolicyChain`] is permissive, so embedders that fully control the host
    /// process can enable fetch without standing up OPA.
    pub policy_chain: Option<Arc<PolicyChain>>,
}

impl FetchOptions {
    /// Allow-all fetch policy with header rules parsed from the JSON config
    /// shape (`--fetch-header-config` format / [`FetchHeaderConfigRule`]).
    pub fn allow_all_with_rules(rules: Vec<FetchHeaderConfigRule>) -> Result<Self> {
        Ok(Self {
            header_rules: rules_from_config(rules)?,
            policy_chain: None,
        })
    }
}

/// Directory-backed heap persistence for an embedded engine. Makes the engine
/// session-capable: `run_js` gains session/heap-snapshot semantics and the
/// MCP surface exposes the session tools.
pub struct HeapOptions {
    /// Directory for content-addressed heap snapshots.
    pub heap_dir: String,
    /// Directory for the session log / heap tag sled databases.
    pub session_db_path: String,
}

/// Build an [`Engine`] from [`EngineOptions`]. [`initialize`] must have been
/// called first.
pub fn build_engine(options: EngineOptions) -> Result<Engine> {
    let EngineOptions {
        heap_memory_max_bytes,
        execution_timeout_secs,
        max_concurrent_executions,
        fetch,
        heap,
        instructions_override,
        run_js_description_override,
    } = options;

    let mut engine = if let Some(heap_options) = heap {
        let storage = AnyHeapStorage::File(FileHeapStorage::new(&heap_options.heap_dir));
        let session_log = SessionLog::new(&heap_options.session_db_path)
            .map_err(|e| anyhow::anyhow!("failed to open session log at {}: {}", heap_options.session_db_path, e))?;
        let heap_tag_db_path = format!("{}/heap-tags", heap_options.session_db_path);
        let heap_tag_store = HeapTagStore::new(&heap_tag_db_path)
            .map_err(|e| anyhow::anyhow!("failed to open heap tag store at {}: {}", heap_tag_db_path, e))?;
        Engine::new_stateful(
            storage,
            Some(session_log),
            Some(heap_tag_store),
            heap_memory_max_bytes,
            execution_timeout_secs,
            max_concurrent_executions,
        )
    } else {
        Engine::new_stateless(
            heap_memory_max_bytes,
            execution_timeout_secs,
            max_concurrent_executions,
        )
    };

    if let Some(fetch_options) = fetch {
        let chain = fetch_options
            .policy_chain
            .unwrap_or_else(|| Arc::new(PolicyChain::new(vec![], EvalMode::All)));
        engine = engine.with_fetch_config(
            FetchConfig::new_with_chain(chain).with_header_rules(fetch_options.header_rules),
        );
    }

    if let Some(instructions) = instructions_override {
        engine = engine.with_instructions_override(instructions);
    }
    if let Some(description) = run_js_description_override {
        engine = engine.with_run_js_description_override(description);
    }

    Ok(engine)
}

/// Build an [`axum::Router`] serving the MCP Streamable HTTP transport for
/// `engine` at the router root. Nest it wherever the host app wants the
/// endpoint to live (e.g. `/mcp/<thread-id>`). Picks the session-capable or
/// stateless MCP service to match the engine, exactly like the standalone
/// binary's `--http-port` transport.
pub fn mcp_http_router(engine: Engine, verifier: Option<Arc<SessionVerifier>>) -> axum::Router {
    if engine.session_capable() {
        let factory_engine = engine.clone();
        let service = StreamableHttpService::new(
            move || Ok(McpService::new(factory_engine.clone(), verifier.clone())),
            LocalSessionManager::default().into(),
            StreamableHttpServerConfig::default(),
        );
        axum::Router::new().fallback_service(service)
    } else {
        let factory_engine = engine.clone();
        let service = StreamableHttpService::new(
            move || Ok(StatelessMcpService::new(factory_engine.clone(), verifier.clone())),
            LocalSessionManager::default().into(),
            StreamableHttpServerConfig::default(),
        );
        axum::Router::new().fallback_service(service)
    }
}
