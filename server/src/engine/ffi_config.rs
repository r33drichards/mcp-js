//! Builder-style configuration for embedding the engine through UniFFI.
//!
//! Every record here derives [`UniffiBuilder`], so each generated language gets
//! a `<Record>Builder` object with chainable setters and a `build()` that
//! reports the first missing required field as
//! [`RuntimeError::MissingRequiredField`]. [`EngineConfig`] is the single input
//! of `Engine::create`; the older `create_stateless` and
//! `create_with_filesystem` constructors are conveniences over it.
//!
//! The axes are independent, as they are for the server: execution limits,
//! hook-gated filesystem access, heap persistence, and filesystem snapshots
//! can each be present or absent. WASM modules are engine-level and cannot be
//! combined with heap persistence, which is enforced at `create`.

use super::ffi::RuntimeError;
use uniffi_builder_derive::UniffiBuilder;

/// Backend for a content-addressed blob store.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum StoreBackend {
    /// A local directory.
    Directory,
    /// An S3 bucket, optionally fronted by a local write-through cache.
    S3,
}

/// Where content-addressed blobs live: heap snapshots, or filesystem snapshot
/// chunks and tree nodes.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record, UniffiBuilder)]
#[uniffi_builder(RuntimeError)]
pub struct BlobStore {
    pub backend: StoreBackend,
    /// `Directory`: the directory holding the blobs (defaults to a path under
    /// the engine's data directory). `S3`: an optional local write-through
    /// cache directory.
    pub path: Option<String>,
    /// `S3` only: the bucket name.
    pub bucket: Option<String>,
}

/// Per-execution resource limits.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record, UniffiBuilder)]
#[uniffi_builder(RuntimeError)]
pub struct ExecutionLimits {
    /// V8 heap limit in MiB (16..=4096).
    pub heap_memory_max_mb: u64,
    /// Default execution deadline in seconds (1..=300); `run_js` may lower it.
    pub execution_timeout_secs: u64,
    /// Concurrent V8 executions (defaults to 1).
    pub max_concurrent_executions: Option<u64>,
}

/// Hook/policy-gated filesystem access for guest `fs.*` and the native `fs_*`
/// methods. Applies to the host filesystem, and to the session overlay when
/// filesystem snapshots are configured.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record, UniffiBuilder)]
#[uniffi_builder(RuntimeError)]
pub struct FilesystemAccess {
    /// The `filesystem` entry of `--policies-json` (`policies`, `pre`,
    /// `stack`), interpreted exactly as the server interprets it. It must
    /// declare some authority: an empty configuration is rejected.
    pub policies_json: String,
    /// With an overlay mounted, let a miss fall through to the host filesystem
    /// as a read-only lower layer. Default false.
    pub passthrough: Option<bool>,
}

/// Everything `Engine::create` needs.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record, UniffiBuilder)]
#[uniffi_builder(RuntimeError)]
pub struct EngineConfig {
    pub limits: ExecutionLimits,
    /// Directory for the session log, execution registry, and default blob
    /// directories. A temporary directory, removed with the engine, when unset.
    pub data_dir: Option<String>,
    pub filesystem: Option<FilesystemAccess>,
    /// Heap persistence: V8 heap snapshots between `run_js` calls, addressed
    /// by content hash and resumed with the `heap` argument.
    pub heap_store: Option<BlobStore>,
    /// Filesystem snapshots: a content-addressed overlay resumed with the
    /// `fs` argument, with movable labels.
    pub fs_snapshot_store: Option<BlobStore>,
    /// Label database for filesystem snapshots (defaults to a path under the
    /// data directory).
    pub fs_labels_db: Option<String>,
}
