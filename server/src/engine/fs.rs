//! Policy-gated filesystem operations for the JavaScript runtime.
//!
//! Provides a Node.js-compatible `fs` API where every operation is evaluated
//! against a [`PolicyChain`] before execution. The chain may contain local
//! Rego files (via regorus) and/or remote OPA servers.
//!
//! Binary data is transferred directly as `Uint8Array` through deno_core's
//! native `#[buffer]` support — no base64 encoding is needed.
//!
//! Available operations (all return Promises):
//! ```js
//! const data = await fs.readFile("/tmp/data.bin");          // Uint8Array (Node default)
//! const text = await fs.readFile("/tmp/data.txt", "utf8");  // string
//! await fs.writeFile("/tmp/out.txt", "hello");              // string data
//! await fs.writeFile("/tmp/out.bin", uint8array);           // binary data
//! await fs.appendFile("/tmp/out.txt", " world");
//! const entries = await fs.readdir("/tmp");                  // string[]
//! const info = await fs.stat("/tmp/data.txt");               // Stats: {size,isFile(),isDirectory(),...}
//! const info = await fs.lstat("/tmp/link");                  // Stats without following symlinks
//! await fs.mkdir("/tmp/newdir", { recursive: true });
//! await fs.rm("/tmp/data.txt");
//! await fs.rm("/tmp/newdir", { recursive: true });
//! await fs.rename("/tmp/old.txt", "/tmp/new.txt");
//! await fs.copyFile("/tmp/a.txt", "/tmp/b.txt");
//! await fs.symlink("/tmp/data.txt", "/tmp/link");            // symlink(target, path)
//! const target = await fs.readlink("/tmp/link");
//! const bool = await fs.exists("/tmp/data.txt");
//! ```
//!
//! ## Node-style compatibility
//!
//! The same methods are also exposed under `fs.promises`, and `fs.stat`/
//! `fs.lstat` return a Node `fs.Stats`-like object with `isFile()`,
//! `isDirectory()`, and `isSymbolicLink()` predicate methods. Errors carry a
//! Node-style `code` (e.g. `ENOENT`, `EEXIST`). Together these let libraries
//! that expect a Node `fs`/`fs.promises` interface — such as `isomorphic-git` —
//! consume the sandbox `fs` object directly.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use deno_core::{JsRuntime, OpState, op2};
use deno_error::JsErrorBox;
use serde::Serialize;

use super::fs_mount::SessionMount;
use super::fs_store::FileWriter;
use super::hooks::{HookChain, PreOutcome};
use super::opa::PolicyChain;
use std::collections::HashMap;
use std::path::Path;

// ── Configuration ────────────────────────────────────────────────────────

/// Handle to the session's active overlay mount. When present in `OpState`, the
/// fs ops delegate to it (after the policy gate) instead of touching the real
/// filesystem. Cheap to clone — it's an `Arc` over the shared mount.
#[derive(Clone)]
pub struct FsMountHandle(pub Arc<tokio::sync::Mutex<SessionMount>>);

impl FsMountHandle {
    pub fn new(mount: SessionMount) -> Self {
        Self(Arc::new(tokio::sync::Mutex::new(mount)))
    }
}

/// Hook capabilities of the filesystem executor: pre hooks may rewrite
/// `path`/`destination` (the effective values are what the operation
/// executes); there is no hookable output, so post hooks are rejected.
pub const HOOK_CAPS: super::hooks::HookCaps = super::hooks::HookCaps {
    input_mutation: true,
    post: false,
};

/// Configuration for the fs module. Stored in deno_core's `OpState`.
#[derive(Clone, Debug)]
pub struct FsConfig {
    pub hooks: Arc<HookChain>,
    pub mcp_headers: Option<serde_json::Value>,
    /// When a per-session overlay mount is attached, controls what happens on an
    /// overlay miss: `false` (default) = overlay-only (the overlay is the whole
    /// fs view; a miss is ENOENT). `true` = overlayfs-style — fall through to the
    /// real filesystem as a read-only lower layer (still policy-gated), so
    /// bundled paths like `/opt/languages` resolve while `/work` stays per-session.
    pub passthrough: bool,
}

impl FsConfig {
    /// Create from a full [`HookChain`] (used with `--policies-json`).
    pub fn new_with_hooks(hooks: Arc<HookChain>) -> Self {
        Self { hooks, mcp_headers: None, passthrough: false }
    }

    /// Create from a bare [`PolicyChain`], wrapped as the sole pre hook.
    pub fn new(chain: Arc<PolicyChain>) -> Self {
        Self::new_with_hooks(Arc::new(HookChain::from_policy("filesystem", chain)))
    }

    pub fn with_mcp_headers(mut self, mcp_headers: Option<serde_json::Value>) -> Self {
        self.mcp_headers = mcp_headers;
        self
    }

    pub fn with_passthrough(mut self, passthrough: bool) -> Self {
        self.passthrough = passthrough;
        self
    }
}

/// Open streaming writers for the current session, keyed by a small integer
/// handle. Stored in `OpState` so a `createWriteStream` handle survives across
/// the separate open / write / close ops.
#[derive(Clone)]
pub struct FsWriters(Arc<tokio::sync::Mutex<FsWritersInner>>);

#[derive(Default)]
struct FsWritersInner {
    next: u32,
    map: HashMap<u32, OpenWrite>,
}

impl Default for FsWriters {
    fn default() -> Self {
        Self(Arc::new(tokio::sync::Mutex::new(FsWritersInner::default())))
    }
}

/// A single open write stream: an overlay file being chunked incrementally, or a
/// real-filesystem file handle.
enum OpenWrite {
    Overlay { path: String, writer: FileWriter },
    Real(tokio::fs::File),
}

// ── Policy input ─────────────────────────────────────────────────────────

#[derive(Serialize)]
struct FsPolicyInput {
    operation: String,
    path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    destination: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    recursive: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    encoding: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mcp_headers: Option<serde_json::Value>,
}

// ── Shared operation service ─────────────────────────────────────────────
//
// Every filesystem operation, whether issued by guest JavaScript through the
// deno ops below or by a native caller through the UniFFI `Engine` methods,
// runs through [`FsService`]: the hook chain evaluates the operation input,
// the effective (possibly rewritten) path and destination are what execute,
// and the same backend selection (session overlay or host filesystem)
// applies. The deno ops only adapt arguments and errors; they add no behavior.

/// Classification of a filesystem failure, stable across backends and
/// transports. The message keeps the guest-visible text, including its
/// Node-style code token, so the JS wrapper and native callers see the same
/// string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsErrorKind {
    NotFound,
    PermissionDenied,
    AlreadyExists,
    NotDirectory,
    IsDirectory,
    NotEmpty,
    InvalidData,
    NotSupported,
    Other,
}

#[derive(Debug, Clone)]
pub struct FsError {
    pub kind: FsErrorKind,
    pub message: String,
}

impl std::fmt::Display for FsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for FsError {}

impl FsError {
    /// A hook-chain failure: a denial, or a chain error that fails closed.
    fn gate(message: String) -> Self {
        let kind = if message.contains(" denied by ") {
            FsErrorKind::PermissionDenied
        } else {
            FsErrorKind::Other
        };
        Self { kind, message }
    }

    fn io(op: &str, path: &str, e: &std::io::Error) -> Self {
        Self { kind: io_kind(e), message: io_err(op, path, e) }
    }

    fn io2(op: &str, from: &str, to: &str, e: &std::io::Error) -> Self {
        Self { kind: io_kind(e), message: io_err2(op, from, to, e) }
    }

    /// An overlay error. The overlay reports Node-style codes as a leading
    /// `CODE:` token, which is preserved and classified.
    fn overlay(op: &str, path: &str, e: impl std::fmt::Display) -> Self {
        let message = format!("fs.{op}: {path}: {e}");
        Self { kind: message_kind(&message), message }
    }

    fn overlay2(op: &str, from: &str, to: &str, e: impl std::fmt::Display) -> Self {
        let message = format!("fs.{op}: {from} -> {to}: {e}");
        Self { kind: message_kind(&message), message }
    }

    fn not_found(op: &str, path: &str) -> Self {
        Self { kind: FsErrorKind::NotFound, message: format!("fs.{op}: {path}: ENOENT") }
    }

    fn invalid_utf8(op: &str, path: &str, e: &std::string::FromUtf8Error) -> Self {
        Self {
            kind: FsErrorKind::InvalidData,
            message: format!("fs.{op}: invalid UTF-8 in {path}: {e}"),
        }
    }

    fn js(self) -> JsErrorBox {
        JsErrorBox::generic(self.message)
    }
}

fn io_kind(e: &std::io::Error) -> FsErrorKind {
    use std::io::ErrorKind::*;
    match e.kind() {
        NotFound => FsErrorKind::NotFound,
        PermissionDenied => FsErrorKind::PermissionDenied,
        AlreadyExists => FsErrorKind::AlreadyExists,
        NotADirectory => FsErrorKind::NotDirectory,
        IsADirectory => FsErrorKind::IsDirectory,
        DirectoryNotEmpty => FsErrorKind::NotEmpty,
        InvalidData => FsErrorKind::InvalidData,
        Unsupported => FsErrorKind::NotSupported,
        _ => FsErrorKind::Other,
    }
}

/// Classify a message by the Node-style code token it carries.
fn message_kind(message: &str) -> FsErrorKind {
    let has = |code: &str| {
        message
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|token| token == code)
    };
    if has("ENOENT") {
        FsErrorKind::NotFound
    } else if has("EACCES") || has("EPERM") {
        FsErrorKind::PermissionDenied
    } else if has("EEXIST") {
        FsErrorKind::AlreadyExists
    } else if has("ENOTDIR") {
        FsErrorKind::NotDirectory
    } else if has("EISDIR") {
        FsErrorKind::IsDirectory
    } else if has("ENOTEMPTY") {
        FsErrorKind::NotEmpty
    } else if has("ENOSYS") {
        FsErrorKind::NotSupported
    } else {
        FsErrorKind::Other
    }
}

/// Metadata for a path, shared by the host and overlay backends. The JS
/// wrapper turns [`FsStat::to_json`] into a Node `fs.Stats`-like object.
#[derive(Debug, Clone)]
pub struct FsStat {
    pub size: u64,
    pub is_file: bool,
    pub is_directory: bool,
    pub is_symlink: bool,
    pub readonly: bool,
    pub mode: u32,
    pub ino: u64,
    pub dev: u64,
    pub nlink: u64,
    pub uid: u32,
    pub gid: u32,
    pub mtime_ms: Option<f64>,
    pub atime_ms: Option<f64>,
    pub ctime_ms: Option<f64>,
    pub birthtime_ms: Option<f64>,
}

impl FsStat {
    fn from_metadata(metadata: &std::fs::Metadata) -> Self {
        let to_ms = |t: std::io::Result<std::time::SystemTime>| {
            t.ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as f64)
        };
        let modified = to_ms(metadata.modified());
        let accessed = to_ms(metadata.accessed());
        let created = to_ms(metadata.created());

        // Unix metadata carries the fields Node consumers read (mode/ino/uid/…);
        // elsewhere synthesize a plausible mode from the file type so callers that
        // derive type from `mode` still classify the entry correctly.
        #[cfg(unix)]
        let (mode, ino, dev, nlink, uid, gid, ctime_ms) = {
            use std::os::unix::fs::MetadataExt;
            let ctime_ms = metadata.ctime() as f64 * 1000.0 + metadata.ctime_nsec() as f64 / 1.0e6;
            (
                metadata.mode(),
                metadata.ino(),
                metadata.dev(),
                metadata.nlink(),
                metadata.uid(),
                metadata.gid(),
                Some(ctime_ms),
            )
        };
        #[cfg(not(unix))]
        let (mode, ino, dev, nlink, uid, gid, ctime_ms): (u32, u64, u64, u64, u32, u32, Option<f64>) = {
            let mode = if metadata.is_dir() {
                0o040755
            } else if metadata.file_type().is_symlink() {
                0o120777
            } else {
                0o100644
            };
            (mode, 0, 0, 1, 0, 0, modified)
        };

        Self {
            size: metadata.len(),
            is_file: metadata.is_file(),
            is_directory: metadata.is_dir(),
            is_symlink: metadata.file_type().is_symlink(),
            readonly: metadata.permissions().readonly(),
            mode,
            ino,
            dev,
            nlink,
            uid,
            gid,
            mtime_ms: modified,
            atime_ms: accessed,
            ctime_ms,
            birthtime_ms: created,
        }
    }

    fn from_mount(s: &super::fs_mount::Stat) -> Self {
        let is_symlink = s.symlink.is_some();
        // Synthesize a mode with the file-type bits set so consumers that derive
        // type from `mode` (e.g. git tree builders) classify it correctly.
        let mode = if s.is_dir {
            0o040000 | (s.mode & 0o777)
        } else if is_symlink {
            0o120000 | (s.mode & 0o777)
        } else {
            0o100000 | (s.mode & 0o777)
        };
        Self {
            size: s.size,
            is_file: !s.is_dir && !is_symlink,
            is_directory: s.is_dir,
            is_symlink,
            readonly: false,
            mode,
            ino: 0,
            dev: 0,
            nlink: 1,
            uid: 0,
            gid: 0,
            mtime_ms: None,
            atime_ms: None,
            ctime_ms: None,
            birthtime_ms: None,
        }
    }

    /// The JSON stat blob `fs.stat` / `fs.lstat` return to the guest.
    pub fn to_json(&self) -> String {
        deno_core::serde_json::json!({
            "size": self.size,
            "isFile": self.is_file,
            "isDirectory": self.is_directory,
            "isSymlink": self.is_symlink,
            "readonly": self.readonly,
            "mode": self.mode,
            "ino": self.ino,
            "dev": self.dev,
            "nlink": self.nlink,
            "uid": self.uid,
            "gid": self.gid,
            "mtimeMs": self.mtime_ms,
            "atimeMs": self.atime_ms,
            "ctimeMs": self.ctime_ms,
            "birthtimeMs": self.birthtime_ms,
        })
        .to_string()
    }
}

/// One filesystem namespace: the hook chain plus the backend it authorizes.
///
/// Overlay-backed services must be driven from the current-thread isolate
/// runtime (the CAS overlay uses deno_unsync); host-backed services may run on
/// any runtime.
#[derive(Clone)]
pub struct FsService {
    config: FsConfig,
    mount: Option<FsMountHandle>,
}

impl FsService {
    pub fn new(config: FsConfig, mount: Option<FsMountHandle>) -> Self {
        Self { config, mount }
    }

    /// A service over the host filesystem only.
    pub fn host(config: FsConfig) -> Self {
        Self::new(config, None)
    }

    pub fn has_mount(&self) -> bool {
        self.mount.is_some()
    }

    async fn gate(
        &self,
        op: &str,
        path: &str,
        destination: Option<&str>,
        recursive: Option<bool>,
        encoding: Option<&str>,
    ) -> Result<FsEffective, FsError> {
        check_policy(
            &self.config.hooks,
            op,
            path,
            destination,
            recursive,
            encoding,
            self.config.mcp_headers.as_ref(),
        )
        .await
        .map_err(FsError::gate)
    }

    /// Gate a two-path operation and return the effective `(path, destination)`.
    /// `check_policy` fails closed if a hook dropped `destination`, so the
    /// fallback never rewrites the operation.
    async fn gate2(&self, op: &str, from: &str, to: &str) -> Result<(String, String), FsError> {
        let eff = self.gate(op, from, Some(to), None, None).await?;
        Ok((eff.path, eff.destination.unwrap_or_else(|| to.to_string())))
    }

    /// Read a file as bytes. `encoding` is the policy input's encoding field
    /// (`"utf8"` or `"buffer"`). Returns the effective path with the content.
    async fn read_bytes(&self, path: &str, encoding: &str) -> Result<(String, Vec<u8>), FsError> {
        let path = self.gate("readFile", path, None, None, Some(encoding)).await?.path;
        if let Some(m) = &self.mount {
            if let Some(content) = m
                .0
                .lock()
                .await
                .read_opt(Path::new(&path))
                .await
                .map_err(|e| FsError::overlay("readFile", &path, e))?
            {
                return Ok((path, content));
            }
            // Overlay miss. With passthrough off (default) the overlay is the whole
            // fs view, so this is ENOENT. With passthrough on, fall through to the
            // real filesystem as a read-only lower layer (already policy-gated above)
            // so bundled paths like /opt/languages resolve while /work stays the
            // per-session overlay.
            if !self.config.passthrough {
                return Err(FsError::not_found("readFile", &path));
            }
            let content = std::fs::read(&path).map_err(|e| FsError::io("readFile", &path, &e))?;
            return Ok((path, content));
        }
        let content = tokio::fs::read(&path)
            .await
            .map_err(|e| FsError::io("readFile", &path, &e))?;
        Ok((path, content))
    }

    pub async fn read_file(&self, path: &str) -> Result<Vec<u8>, FsError> {
        Ok(self.read_bytes(path, "buffer").await?.1)
    }

    pub async fn read_text(&self, path: &str) -> Result<String, FsError> {
        let (path, content) = self.read_bytes(path, "utf8").await?;
        String::from_utf8(content).map_err(|e| FsError::invalid_utf8("readFile", &path, &e))
    }

    /// Read at most `max_bytes` bytes starting at `offset`, gated as a
    /// `readFile`. Returns fewer bytes only at end of file. Lets a caller page
    /// through a large file without loading it whole; the overlay backend has
    /// no partial read, so it slices the full content.
    pub async fn read_range(&self, path: &str, offset: u64, max_bytes: u64) -> Result<Vec<u8>, FsError> {
        let path = self.gate("readFile", path, None, None, Some("buffer")).await?.path;
        let max = usize::try_from(max_bytes).unwrap_or(usize::MAX);
        if let Some(m) = &self.mount {
            let content = m
                .0
                .lock()
                .await
                .read_opt(Path::new(&path))
                .await
                .map_err(|e| FsError::overlay("readFile", &path, e))?;
            let content = match content {
                Some(content) => content,
                None if self.config.passthrough => {
                    std::fs::read(&path).map_err(|e| FsError::io("readFile", &path, &e))?
                }
                None => return Err(FsError::not_found("readFile", &path)),
            };
            let start = usize::try_from(offset).unwrap_or(usize::MAX).min(content.len());
            let end = start.saturating_add(max).min(content.len());
            return Ok(content[start..end].to_vec());
        }
        use tokio::io::{AsyncReadExt, AsyncSeekExt};
        let mut file = tokio::fs::File::open(&path)
            .await
            .map_err(|e| FsError::io("readFile", &path, &e))?;
        if file.metadata().await.map_err(|e| FsError::io("readFile", &path, &e))?.is_dir() {
            let e = std::io::Error::from(std::io::ErrorKind::IsADirectory);
            return Err(FsError::io("readFile", &path, &e));
        }
        file.seek(std::io::SeekFrom::Start(offset))
            .await
            .map_err(|e| FsError::io("readFile", &path, &e))?;
        let mut out = Vec::new();
        file.take(max as u64)
            .read_to_end(&mut out)
            .await
            .map_err(|e| FsError::io("readFile", &path, &e))?;
        Ok(out)
    }

    /// The canonical path of an existing path, resolving symlinks. Gated as a
    /// `stat`, which is the guest operation with the same follow semantics.
    pub async fn canonical_path(&self, path: &str) -> Result<String, FsError> {
        let path = self.gate("stat", path, None, None, None).await?.path;
        if let Some(m) = &self.mount {
            // The overlay has no symlink resolution; it reports the stat'ed path.
            m.0.lock()
                .await
                .stat(Path::new(&path))
                .await
                .map_err(|e| FsError::overlay("stat", &path, e))?;
            return Ok(path);
        }
        let canonical = tokio::fs::canonicalize(&path)
            .await
            .map_err(|e| FsError::io("stat", &path, &e))?;
        Ok(canonical.to_string_lossy().into_owned())
    }

    pub async fn write_file(&self, path: &str, data: &[u8]) -> Result<(), FsError> {
        let path = self.gate("writeFile", path, None, None, None).await?.path;
        if let Some(m) = &self.mount {
            return m
                .0
                .lock()
                .await
                .write(Path::new(&path), data)
                .await
                .map_err(|e| FsError::overlay("writeFile", &path, e));
        }
        tokio::fs::write(&path, data)
            .await
            .map_err(|e| FsError::io("writeFile", &path, &e))
    }

    pub async fn append_file(&self, path: &str, data: &[u8]) -> Result<(), FsError> {
        let path = self.gate("appendFile", path, None, None, None).await?.path;
        if let Some(m) = &self.mount {
            let mut guard = m.0.lock().await;
            let mut existing = guard.read(Path::new(&path)).await.unwrap_or_default();
            existing.extend_from_slice(data);
            return guard
                .write(Path::new(&path), &existing)
                .await
                .map_err(|e| FsError::overlay("appendFile", &path, e));
        }
        use tokio::io::AsyncWriteExt;
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await
            .map_err(|e| FsError::io("appendFile", &path, &e))?;
        file.write_all(data)
            .await
            .map_err(|e| FsError::io("appendFile", &path, &e))?;
        // tokio's File buffers writes; without a flush the append may still be
        // in flight when this returns and an immediate read sees the old file.
        file.flush()
            .await
            .map_err(|e| FsError::io("appendFile", &path, &e))
    }

    pub async fn readdir(&self, path: &str) -> Result<Vec<String>, FsError> {
        let path = self.gate("readdir", path, None, None, None).await?.path;
        if let Some(m) = &self.mount {
            return m
                .0
                .lock()
                .await
                .readdir(Path::new(&path))
                .await
                .map_err(|e| FsError::overlay("readdir", &path, e));
        }
        let mut entries = Vec::new();
        let mut dir = tokio::fs::read_dir(&path)
            .await
            .map_err(|e| FsError::io("readdir", &path, &e))?;
        while let Some(entry) = dir
            .next_entry()
            .await
            .map_err(|e| FsError::io("readdir", &path, &e))?
        {
            if let Some(name) = entry.file_name().to_str() {
                entries.push(name.to_string());
            }
        }
        Ok(entries)
    }

    /// Metadata for a path. `follow` selects Node `stat` (follow a final
    /// symlink) versus `lstat`; the overlay never follows symlinks, so its
    /// stat already has lstat semantics.
    pub async fn stat(&self, path: &str, follow: bool) -> Result<FsStat, FsError> {
        let op = if follow { "stat" } else { "lstat" };
        let path = self.gate(op, path, None, None, None).await?.path;
        if let Some(m) = &self.mount {
            let s = m
                .0
                .lock()
                .await
                .stat(Path::new(&path))
                .await
                .map_err(|e| FsError::overlay(op, &path, e))?;
            return Ok(FsStat::from_mount(&s));
        }
        let metadata = if follow {
            tokio::fs::metadata(&path).await
        } else {
            tokio::fs::symlink_metadata(&path).await
        }
        .map_err(|e| FsError::io(op, &path, &e))?;
        Ok(FsStat::from_metadata(&metadata))
    }

    pub async fn readlink(&self, path: &str) -> Result<String, FsError> {
        let path = self.gate("readlink", path, None, None, None).await?.path;
        if let Some(m) = &self.mount {
            let target = m
                .0
                .lock()
                .await
                .readlink(Path::new(&path))
                .await
                .map_err(|e| FsError::overlay("readlink", &path, e))?;
            return Ok(target.to_string_lossy().into_owned());
        }
        let target = tokio::fs::read_link(&path)
            .await
            .map_err(|e| FsError::io("readlink", &path, &e))?;
        Ok(target.to_string_lossy().into_owned())
    }

    /// Create a symlink at `link` pointing to `target` (Node `fs.symlink(target, path)`).
    /// The policy gates on the link path being created; the target is carried as
    /// the destination so a policy can constrain both sides.
    pub async fn symlink(&self, target: &str, link: &str) -> Result<(), FsError> {
        let (link, target) = self.gate2("symlink", link, target).await?;
        if let Some(m) = &self.mount {
            return m
                .0
                .lock()
                .await
                .symlink(Path::new(&target), Path::new(&link))
                .await
                .map_err(|e| FsError::overlay2("symlink", &link, &target, e));
        }
        symlink_impl(&target, &link)
            .await
            .map_err(|e| FsError::io2("symlink", &link, &target, &e))
    }

    pub async fn mkdir(&self, path: &str, recursive: bool) -> Result<(), FsError> {
        let path = self.gate("mkdir", path, None, Some(recursive), None).await?.path;
        if let Some(m) = &self.mount {
            return m
                .0
                .lock()
                .await
                .mkdir(Path::new(&path))
                .await
                .map_err(|e| FsError::overlay("mkdir", &path, e));
        }
        if recursive {
            tokio::fs::create_dir_all(&path).await
        } else {
            tokio::fs::create_dir(&path).await
        }
        .map_err(|e| FsError::io("mkdir", &path, &e))
    }

    pub async fn rm(&self, path: &str, recursive: bool) -> Result<(), FsError> {
        let path = self.gate("rm", path, None, Some(recursive), None).await?.path;
        if let Some(m) = &self.mount {
            return m
                .0
                .lock()
                .await
                .remove(Path::new(&path), recursive)
                .await
                .map_err(|e| FsError::overlay("rm", &path, e));
        }
        let metadata = tokio::fs::metadata(&path)
            .await
            .map_err(|e| FsError::io("rm", &path, &e))?;
        if metadata.is_dir() {
            if recursive {
                tokio::fs::remove_dir_all(&path).await
            } else {
                tokio::fs::remove_dir(&path).await
            }
        } else {
            tokio::fs::remove_file(&path).await
        }
        .map_err(|e| FsError::io("rm", &path, &e))
    }

    pub async fn rename(&self, from: &str, to: &str) -> Result<(), FsError> {
        let (from, to) = self.gate2("rename", from, to).await?;
        if let Some(m) = &self.mount {
            return m
                .0
                .lock()
                .await
                .rename(Path::new(&from), Path::new(&to))
                .await
                .map_err(|e| FsError::overlay2("rename", &from, &to, e));
        }
        tokio::fs::rename(&from, &to)
            .await
            .map_err(|e| FsError::io2("rename", &from, &to, &e))
    }

    pub async fn copy_file(&self, from: &str, to: &str) -> Result<(), FsError> {
        let (from, to) = self.gate2("copyFile", from, to).await?;
        if let Some(m) = &self.mount {
            // Copy by reference: clones the content-addressed entry, no rechunk.
            return m
                .0
                .lock()
                .await
                .copy(Path::new(&from), Path::new(&to))
                .await
                .map_err(|e| FsError::overlay2("copyFile", &from, &to, e));
        }
        tokio::fs::copy(&from, &to)
            .await
            .map(|_| ())
            .map_err(|e| FsError::io2("copyFile", &from, &to, &e))
    }

    pub async fn exists(&self, path: &str) -> Result<bool, FsError> {
        let path = self.gate("exists", path, None, None, None).await?.path;
        if let Some(m) = &self.mount {
            return Ok(m.0.lock().await.exists(Path::new(&path)).await);
        }
        Ok(tokio::fs::try_exists(&path).await.unwrap_or(false))
    }
}

#[cfg(unix)]
async fn symlink_impl(target: &str, link: &str) -> std::io::Result<()> {
    tokio::fs::symlink(target, link).await
}

#[cfg(not(unix))]
async fn symlink_impl(_target: &str, _link: &str) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "symlinks are not supported on this platform",
    ))
}

// ── Async deno_core ops ──────────────────────────────────────────────────
//
// Each op resolves the session's service and runs one operation. Overlay-backed
// services run inline: the CAS overlay uses deno_unsync, which asserts the
// current-thread isolate runtime, and tokio::spawn would move the work onto the
// multi-thread runtime and abort the process. Host-backed services are
// offloaded so blocking-ish host I/O does not stall the isolate.

fn service(state: &Rc<RefCell<OpState>>) -> Result<FsService, JsErrorBox> {
    Ok(FsService::new(extract_config(state)?, extract_mount(state)))
}

async fn run_op<T, F, Fut>(state: &Rc<RefCell<OpState>>, op: F) -> Result<T, JsErrorBox>
where
    T: Send + 'static,
    F: FnOnce(FsService) -> Fut,
    Fut: std::future::Future<Output = Result<T, FsError>> + Send + 'static,
{
    let service = service(state)?;
    if service.has_mount() {
        return op(service).await.map_err(FsError::js);
    }
    tokio::spawn(op(service))
        .await
        .map_err(|e| JsErrorBox::generic(format!("fs task join error: {}", e)))?
        .map_err(FsError::js)
}

const EMPTY_OBJECT: &str = "{}";

/// Read a file as UTF-8 text.
#[op2(async)]
#[string]
async fn op_fs_read_file_text(
    state: Rc<RefCell<OpState>>,
    #[string] path: String,
) -> Result<String, JsErrorBox> {
    run_op(&state, |fs| async move { fs.read_text(&path).await }).await
}

/// Read a file as raw bytes, returned as a Uint8Array to JavaScript.
#[op2(async)]
#[buffer]
async fn op_fs_read_file_buffer(
    state: Rc<RefCell<OpState>>,
    #[string] path: String,
) -> Result<Vec<u8>, JsErrorBox> {
    run_op(&state, |fs| async move { fs.read_file(&path).await }).await
}

/// Write a file from a UTF-8 string.
#[op2(async)]
#[string]
async fn op_fs_write_file_text(
    state: Rc<RefCell<OpState>>,
    #[string] path: String,
    #[string] data: String,
) -> Result<String, JsErrorBox> {
    run_op(&state, |fs| async move {
        fs.write_file(&path, data.as_bytes()).await?;
        Ok(EMPTY_OBJECT.to_string())
    })
    .await
}

/// Write a file from raw bytes (Uint8Array from JavaScript).
#[op2(async)]
#[string]
async fn op_fs_write_file_buffer(
    state: Rc<RefCell<OpState>>,
    #[string] path: String,
    #[buffer(copy)] data: Vec<u8>,
) -> Result<String, JsErrorBox> {
    run_op(&state, |fs| async move {
        fs.write_file(&path, &data).await?;
        Ok(EMPTY_OBJECT.to_string())
    })
    .await
}

/// Append to a file.
#[op2(async)]
#[string]
async fn op_fs_append_file(
    state: Rc<RefCell<OpState>>,
    #[string] path: String,
    #[string] data: String,
) -> Result<String, JsErrorBox> {
    run_op(&state, |fs| async move {
        fs.append_file(&path, data.as_bytes()).await?;
        Ok(EMPTY_OBJECT.to_string())
    })
    .await
}

/// Read a directory. Returns JSON array of entry names.
#[op2(async)]
#[string]
async fn op_fs_readdir(
    state: Rc<RefCell<OpState>>,
    #[string] path: String,
) -> Result<String, JsErrorBox> {
    run_op(&state, |fs| async move {
        let names = fs.readdir(&path).await?;
        Ok(deno_core::serde_json::json!(names).to_string())
    })
    .await
}

/// Stat a path. Returns JSON with size, isFile, isDirectory, etc.
#[op2(async)]
#[string]
async fn op_fs_stat(
    state: Rc<RefCell<OpState>>,
    #[string] path: String,
) -> Result<String, JsErrorBox> {
    run_op(&state, |fs| async move { Ok(fs.stat(&path, true).await?.to_json()) }).await
}

/// Stat a path **without** following a final symlink (Node `fs.lstat`).
#[op2(async)]
#[string]
async fn op_fs_lstat(
    state: Rc<RefCell<OpState>>,
    #[string] path: String,
) -> Result<String, JsErrorBox> {
    run_op(&state, |fs| async move { Ok(fs.stat(&path, false).await?.to_json()) }).await
}

/// Read a symlink's target, returned as a string.
#[op2(async)]
#[string]
async fn op_fs_readlink(
    state: Rc<RefCell<OpState>>,
    #[string] path: String,
) -> Result<String, JsErrorBox> {
    run_op(&state, |fs| async move { fs.readlink(&path).await }).await
}

/// Create a symlink at `link` pointing to `target` (Node `fs.symlink(target, path)`).
#[op2(async)]
#[string]
async fn op_fs_symlink(
    state: Rc<RefCell<OpState>>,
    #[string] target: String,
    #[string] link: String,
) -> Result<String, JsErrorBox> {
    run_op(&state, |fs| async move {
        fs.symlink(&target, &link).await?;
        Ok(EMPTY_OBJECT.to_string())
    })
    .await
}

/// Create a directory.
#[op2(async)]
#[string]
async fn op_fs_mkdir(
    state: Rc<RefCell<OpState>>,
    #[string] path: String,
    #[smi] recursive: i32,
) -> Result<String, JsErrorBox> {
    run_op(&state, |fs| async move {
        fs.mkdir(&path, recursive != 0).await?;
        Ok(EMPTY_OBJECT.to_string())
    })
    .await
}

/// Remove a file or directory.
#[op2(async)]
#[string]
async fn op_fs_rm(
    state: Rc<RefCell<OpState>>,
    #[string] path: String,
    #[smi] recursive: i32,
) -> Result<String, JsErrorBox> {
    run_op(&state, |fs| async move {
        fs.rm(&path, recursive != 0).await?;
        Ok(EMPTY_OBJECT.to_string())
    })
    .await
}

/// Rename (move) a file or directory.
#[op2(async)]
#[string]
async fn op_fs_rename(
    state: Rc<RefCell<OpState>>,
    #[string] from: String,
    #[string] to: String,
) -> Result<String, JsErrorBox> {
    run_op(&state, |fs| async move {
        fs.rename(&from, &to).await?;
        Ok(EMPTY_OBJECT.to_string())
    })
    .await
}

/// Copy a file.
#[op2(async)]
#[string]
async fn op_fs_copy_file(
    state: Rc<RefCell<OpState>>,
    #[string] from: String,
    #[string] to: String,
) -> Result<String, JsErrorBox> {
    run_op(&state, |fs| async move {
        fs.copy_file(&from, &to).await?;
        Ok(EMPTY_OBJECT.to_string())
    })
    .await
}

/// Check if a path exists.
#[op2(async)]
#[string]
async fn op_fs_exists(
    state: Rc<RefCell<OpState>>,
    #[string] path: String,
) -> Result<String, JsErrorBox> {
    run_op(&state, |fs| async move {
        Ok(if fs.exists(&path).await? { "true" } else { "false" }.to_string())
    })
    .await
}

// ── Streaming writes ─────────────────────────────────────────────────────

/// Open a streaming write to `path`, returning a small integer handle. Bytes are
/// fed incrementally (chunked on the fly), so a multi-GB file never has to exist
/// in memory all at once.
#[op2(async)]
#[smi]
async fn op_fs_write_stream_open(
    state: Rc<RefCell<OpState>>,
    #[string] path: String,
) -> Result<u32, JsErrorBox> {
    let config = extract_config(&state)?;
    let mount = extract_mount(&state);
    let writers = extract_writers(&state)?;

    // Mount branch runs inline (current-thread isolate runtime; deno_unsync needs it).
    if let Some(m) = mount {
        let path = check_policy(&config.hooks, "writeFile", &path, None, None, None, config.mcp_headers.as_ref())
            .await
            .map_err(JsErrorBox::generic)?
            .path;
        let store = m.0.lock().await.store_handle();
        let ow = OpenWrite::Overlay { path: path.clone(), writer: FileWriter::new(store) };
        let mut g = writers.0.lock().await;
        let id = g.next;
        g.next = g.next.wrapping_add(1);
        g.map.insert(id, ow);
        return Ok(id);
    }

    tokio::spawn(async move {
        let path = check_policy(&config.hooks, "writeFile", &path, None, None, None, config.mcp_headers.as_ref()).await?.path;

        let f = tokio::fs::File::create(&path).await
            .map_err(|e| io_err("createWriteStream", &path, &e))?;
        let ow = OpenWrite::Real(f);
        let mut g = writers.0.lock().await;
        let id = g.next;
        g.next = g.next.wrapping_add(1);
        g.map.insert(id, ow);
        Ok::<u32, String>(id)
    })
    .await
    .map_err(|e| JsErrorBox::generic(format!("fs task join error: {}", e)))?
    .map_err(|e: String| JsErrorBox::generic(e))
}

/// Feed a chunk of bytes to an open write stream.
#[op2(async)]
#[string]
async fn op_fs_write_stream_chunk_buffer(
    state: Rc<RefCell<OpState>>,
    #[smi] id: u32,
    #[buffer(copy)] data: Vec<u8>,
) -> Result<String, JsErrorBox> {
    let writers = extract_writers(&state)?;
    // Run inline: the overlay FileWriter uses deno_unsync, which requires the
    // current-thread isolate runtime; tokio::spawn would abort the process.
    feed_stream(&writers, id, &data).await.map_err(JsErrorBox::generic)
}

/// Feed a chunk of text to an open write stream.
#[op2(async)]
#[string]
async fn op_fs_write_stream_chunk_text(
    state: Rc<RefCell<OpState>>,
    #[smi] id: u32,
    #[string] data: String,
) -> Result<String, JsErrorBox> {
    let writers = extract_writers(&state)?;
    // Run inline: the overlay FileWriter uses deno_unsync, which requires the
    // current-thread isolate runtime; tokio::spawn would abort the process.
    feed_stream(&writers, id, data.as_bytes()).await.map_err(JsErrorBox::generic)
}

/// Finish an open write stream: flush the final chunk and install the file.
#[op2(async)]
#[string]
async fn op_fs_write_stream_close(
    state: Rc<RefCell<OpState>>,
    #[smi] id: u32,
) -> Result<String, JsErrorBox> {
    let mount = extract_mount(&state);
    let writers = extract_writers(&state)?;
    // Run inline: the overlay writer.finish() / put_entry path uses deno_unsync,
    // which requires the current-thread isolate runtime; tokio::spawn would abort.
    let ow = writers
        .0
        .lock()
        .await
        .map
        .remove(&id)
        .ok_or_else(|| JsErrorBox::generic("fs write stream: invalid handle".to_string()))?;
    match ow {
        OpenWrite::Overlay { path, writer } => {
            let entry = writer.finish().await.map_err(|e| JsErrorBox::generic(e.to_string()))?;
            if let Some(m) = mount {
                m.0.lock().await.put_entry(Path::new(&path), entry);
            }
        }
        OpenWrite::Real(mut f) => {
            use tokio::io::AsyncWriteExt;
            f.flush().await.map_err(|e| JsErrorBox::generic(e.to_string()))?;
        }
    }
    Ok("{}".to_string())
}

/// Feed bytes to writer `id`: take it out of the registry, feed, put it back, so
/// the registry lock is never held across the await.
async fn feed_stream(writers: &FsWriters, id: u32, data: &[u8]) -> Result<String, String> {
    let mut ow = writers
        .0
        .lock()
        .await
        .map
        .remove(&id)
        .ok_or_else(|| "fs write stream: invalid handle".to_string())?;
    let res = match &mut ow {
        OpenWrite::Overlay { writer, .. } => writer.feed(data).await.map_err(|e| e.to_string()),
        OpenWrite::Real(f) => {
            use tokio::io::AsyncWriteExt;
            f.write_all(data).await.map_err(|e| e.to_string())
        }
    };
    writers.0.lock().await.map.insert(id, ow);
    res?;
    Ok("{}".to_string())
}

// ── Extension registration ──────────────────────────────────────────────

deno_core::extension!(
    fs_ext,
    ops = [
        op_fs_read_file_text,
        op_fs_read_file_buffer,
        op_fs_write_file_text,
        op_fs_write_file_buffer,
        op_fs_append_file,
        op_fs_readdir,
        op_fs_stat,
        op_fs_lstat,
        op_fs_readlink,
        op_fs_symlink,
        op_fs_mkdir,
        op_fs_rm,
        op_fs_rename,
        op_fs_copy_file,
        op_fs_exists,
        op_fs_write_stream_open,
        op_fs_write_stream_chunk_buffer,
        op_fs_write_stream_chunk_text,
        op_fs_write_stream_close,
    ],
    state = |state| {
        state.put(FsWriters::default());
    },
);

pub fn create_extension() -> deno_core::Extension {
    fs_ext::init()
}

// ── Inject fs JS wrapper into the global scope ──────────────────────────

pub fn inject_fs(runtime: &mut JsRuntime) -> Result<(), String> {
    runtime
        .execute_script("<fs-setup>", FS_JS_WRAPPER.to_string())
        .map_err(|e| format!("Failed to install fs wrapper: {}", e))?;
    Ok(())
}

const FS_JS_WRAPPER: &str = r#"
(function() {
    const ops = Deno.core.ops;

    // Known Node.js filesystem error codes. The Rust ops embed the matching
    // token in their error message; we surface it as `err.code` so callers that
    // branch on it (isomorphic-git, graceful-fs, …) behave as they do on Node.
    const FS_CODES = /\b(ENOENT|EEXIST|EACCES|EPERM|ENOTDIR|EISDIR|ENOTEMPTY|EROFS|ELOOP|EINVAL|EXDEV|ENOSPC|EMFILE|ENFILE|EBADF|ENOSYS)\b/;
    function tagError(e) {
        try {
            if (e && typeof e === 'object' && (e.code === undefined || e.code === null)) {
                const msg = typeof e.message === 'string' ? e.message : String(e);
                const m = msg.match(FS_CODES);
                if (m) e.code = m[1];
            }
        } catch (_) { /* never let tagging mask the original error */ }
        return e;
    }
    async function call(name, ...args) {
        try {
            return await ops[name](...args);
        } catch (e) {
            throw tagError(e);
        }
    }

    // A Node `fs.Stats`-like object: the data fields plus the is*() predicate
    // methods consumers call. Built from the JSON the stat/lstat ops return.
    function makeStats(o) {
        const n = (v) => (typeof v === 'number' ? v : 0);
        const size = n(o.size);
        const stats = {
            dev: n(o.dev), ino: n(o.ino), mode: n(o.mode), nlink: n(o.nlink) || 1,
            uid: n(o.uid), gid: n(o.gid), rdev: 0,
            size: size, blksize: 4096, blocks: Math.ceil(size / 512),
            atimeMs: n(o.atimeMs), mtimeMs: n(o.mtimeMs),
            ctimeMs: n(o.ctimeMs), birthtimeMs: n(o.birthtimeMs),
            atime: new Date(n(o.atimeMs)), mtime: new Date(n(o.mtimeMs)),
            ctime: new Date(n(o.ctimeMs)), birthtime: new Date(n(o.birthtimeMs)),
            readonly: !!o.readonly,
        };
        const isFile = !!o.isFile, isDirectory = !!o.isDirectory, isSymbolicLink = !!o.isSymlink;
        stats.isFile = function() { return isFile; };
        stats.isDirectory = function() { return isDirectory; };
        stats.isSymbolicLink = function() { return isSymbolicLink; };
        stats.isBlockDevice = function() { return false; };
        stats.isCharacterDevice = function() { return false; };
        stats.isFIFO = function() { return false; };
        stats.isSocket = function() { return false; };
        return stats;
    }

    // Node's readFile encoding argument: a string ('utf8') or an options object
    // ({ encoding }). Returns null for "no encoding given".
    function readEncoding(opt) {
        if (typeof opt === 'string') return opt;
        if (opt && typeof opt === 'object' && opt.encoding) return opt.encoding;
        return null;
    }

    async function readFileText(path) {
        if (typeof path !== 'string') throw new TypeError('fs.readFile: path must be a string');
        return await call('op_fs_read_file_text', path);
    }
    async function readFileBuffer(path) {
        if (typeof path !== 'string') throw new TypeError('fs.readFile: path must be a string');
        return await call('op_fs_read_file_buffer', path);
    }

    // Node's readFile contract: a Uint8Array by default, a string when a text
    // encoding ('utf8', 'utf-8', …) is supplied. fs.readFile and
    // fs.promises.readFile share this single behavior, so binary reads (e.g.
    // isomorphic-git git objects) are lossless and text reads opt in via an
    // encoding argument — exactly as Node behaves.
    async function readFile(path, options) {
        const enc = readEncoding(options);
        if (enc && enc !== 'buffer') return await readFileText(path);
        return await readFileBuffer(path);
    }

    async function writeFile(path, data) {
        if (typeof path !== 'string') throw new TypeError('fs.writeFile: path must be a string');
        if (data instanceof Uint8Array) {
            await call('op_fs_write_file_buffer', path, data);
        } else if (ArrayBuffer.isView(data)) {
            await call('op_fs_write_file_buffer', path, new Uint8Array(data.buffer, data.byteOffset, data.byteLength));
        } else if (data instanceof ArrayBuffer) {
            await call('op_fs_write_file_buffer', path, new Uint8Array(data));
        } else {
            await call('op_fs_write_file_text', path, String(data));
        }
    }

    async function appendFile(path, data) {
        if (typeof path !== 'string') throw new TypeError('fs.appendFile: path must be a string');
        await call('op_fs_append_file', path, String(data));
    }

    async function readdir(path) {
        if (typeof path !== 'string') throw new TypeError('fs.readdir: path must be a string');
        return JSON.parse(await call('op_fs_readdir', path));
    }

    async function stat(path) {
        if (typeof path !== 'string') throw new TypeError('fs.stat: path must be a string');
        return makeStats(JSON.parse(await call('op_fs_stat', path)));
    }

    async function lstat(path) {
        if (typeof path !== 'string') throw new TypeError('fs.lstat: path must be a string');
        return makeStats(JSON.parse(await call('op_fs_lstat', path)));
    }

    async function mkdir(path, options) {
        if (typeof path !== 'string') throw new TypeError('fs.mkdir: path must be a string');
        await call('op_fs_mkdir', path, (options && options.recursive) ? 1 : 0);
    }

    async function rm(path, options) {
        if (typeof path !== 'string') throw new TypeError('fs.rm: path must be a string');
        await call('op_fs_rm', path, (options && options.recursive) ? 1 : 0);
    }

    // Node's fs.rmdir; recursive removal of a directory tree when requested.
    async function rmdir(path, options) {
        if (typeof path !== 'string') throw new TypeError('fs.rmdir: path must be a string');
        await call('op_fs_rm', path, (options && options.recursive) ? 1 : 0);
    }

    async function unlink(path) {
        if (typeof path !== 'string') throw new TypeError('fs.unlink: path must be a string');
        await call('op_fs_rm', path, 0);
    }

    async function rename(oldPath, newPath) {
        if (typeof oldPath !== 'string') throw new TypeError('fs.rename: oldPath must be a string');
        if (typeof newPath !== 'string') throw new TypeError('fs.rename: newPath must be a string');
        await call('op_fs_rename', oldPath, newPath);
    }

    async function copyFile(src, dest) {
        if (typeof src !== 'string') throw new TypeError('fs.copyFile: src must be a string');
        if (typeof dest !== 'string') throw new TypeError('fs.copyFile: dest must be a string');
        await call('op_fs_copy_file', src, dest);
    }

    async function readlink(path) {
        if (typeof path !== 'string') throw new TypeError('fs.readlink: path must be a string');
        return await call('op_fs_readlink', path);
    }

    // Node signature: symlink(target, path) creates a link at `path` -> `target`.
    async function symlink(target, path) {
        if (typeof target !== 'string') throw new TypeError('fs.symlink: target must be a string');
        if (typeof path !== 'string') throw new TypeError('fs.symlink: path must be a string');
        await call('op_fs_symlink', target, path);
    }

    async function exists(path) {
        if (typeof path !== 'string') throw new TypeError('fs.exists: path must be a string');
        return (await call('op_fs_exists', path)) === 'true';
    }

    // Streaming write handle: feed a large file in pieces so neither JS nor the
    // runtime ever holds the whole thing. The file becomes visible only after
    // close().
    async function createWriteStream(path) {
        if (typeof path !== 'string') throw new TypeError('fs.createWriteStream: path must be a string');
        const id = await call('op_fs_write_stream_open', path);
        let closed = false;
        return {
            write: async function(chunk) {
                if (closed) throw new Error('fs.createWriteStream: write after close');
                if (chunk instanceof Uint8Array) {
                    await call('op_fs_write_stream_chunk_buffer', id, chunk);
                } else {
                    await call('op_fs_write_stream_chunk_text', id, String(chunk));
                }
            },
            close: async function() {
                if (closed) return;
                closed = true;
                await call('op_fs_write_stream_close', id);
            },
        };
    }

    // The promise-based surface mirroring Node's `fs.promises`. readFile follows
    // Node semantics here (bytes by default). Libraries that wrap a filesystem —
    // isomorphic-git among them — detect this enumerable property and bind its
    // methods, so every method they look up must exist.
    const promises = {
        readFile,
        writeFile, appendFile, readdir, stat, lstat, mkdir, rm, rmdir,
        unlink, rename, copyFile, readlink, symlink, exists,
    };

    globalThis.fs = {
        readFile, writeFile, appendFile, readdir, stat, lstat, mkdir, rm, rmdir,
        unlink, rename, copyFile, readlink, symlink, exists, createWriteStream,
        promises,
    };
})();
"#;

// ── Helpers ──────────────────────────────────────────────────────────────

fn extract_config(state: &Rc<RefCell<OpState>>) -> Result<FsConfig, JsErrorBox> {
    let state = state.borrow();
    let config = state.try_borrow::<FsConfig>()
        .ok_or_else(|| JsErrorBox::generic("fs: internal error — no fs config available"))?;
    Ok(config.clone())
}

/// The active mount, if any. When `Some`, fs ops operate on the virtual overlay
/// rather than the host filesystem.
fn extract_mount(state: &Rc<RefCell<OpState>>) -> Option<FsMountHandle> {
    state.borrow().try_borrow::<FsMountHandle>().cloned()
}

/// The session's streaming-write registry (installed by the extension's `state`
/// initializer, so it is always present).
fn extract_writers(state: &Rc<RefCell<OpState>>) -> Result<FsWriters, JsErrorBox> {
    state
        .borrow()
        .try_borrow::<FsWriters>()
        .cloned()
        .ok_or_else(|| JsErrorBox::generic("fs: internal error — no write-stream registry"))
}

/// Map a `std::io::Error` to a Node.js-style error code (`ENOENT`, `EEXIST`,
/// …). Returns `None` for kinds without a well-known POSIX name. The JS wrapper
/// surfaces this token as `err.code` so callers that branch on it (isomorphic-git,
/// etc.) behave the same as on Node.
fn io_code(e: &std::io::Error) -> Option<&'static str> {
    use std::io::ErrorKind::*;
    Some(match e.kind() {
        NotFound => "ENOENT",
        PermissionDenied => "EACCES",
        AlreadyExists => "EEXIST",
        NotADirectory => "ENOTDIR",
        IsADirectory => "EISDIR",
        DirectoryNotEmpty => "ENOTEMPTY",
        ReadOnlyFilesystem => "EROFS",
        Unsupported => "ENOSYS",
        _ => return None,
    })
}

/// Format a single-path fs io error with the Node-style code embedded as a
/// token the JS wrapper can extract: `fs.<op>: <path>: <CODE>: <message>` (the
/// `<CODE>:` segment is omitted when the kind is unmapped).
fn io_err(op: &str, path: &str, e: &std::io::Error) -> String {
    match io_code(e) {
        Some(code) => format!("fs.{op}: {path}: {code}: {e}"),
        None => format!("fs.{op}: {path}: {e}"),
    }
}

/// Like [`io_err`] but for two-path operations (`rename`, `copyFile`, `symlink`).
fn io_err2(op: &str, from: &str, to: &str, e: &std::io::Error) -> String {
    match io_code(e) {
        Some(code) => format!("fs.{op}: {from} -> {to}: {code}: {e}"),
        None => format!("fs.{op}: {from} -> {to}: {e}"),
    }
}

/// The fields a pre hook may have mutated, extracted back out of the
/// effective hook-chain input. Only `path` and `destination` feed back into
/// the operation; other input fields are context.
#[derive(serde::Deserialize)]
struct FsEffective {
    path: String,
    #[serde(default)]
    destination: Option<String>,
}

async fn check_policy(
    hooks: &HookChain,
    operation: &str,
    path: &str,
    destination: Option<&str>,
    recursive: Option<bool>,
    encoding: Option<&str>,
    mcp_headers: Option<&serde_json::Value>,
) -> Result<FsEffective, String> {
    let input = FsPolicyInput {
        operation: operation.to_string(),
        path: path.to_string(),
        destination: destination.map(|s| s.to_string()),
        recursive,
        encoding: encoding.map(|s| s.to_string()),
        mcp_headers: mcp_headers.cloned(),
    };

    let input_value = serde_json::to_value(&input)
        .map_err(|e| format!("fs.{}: failed to serialize policy input: {}", operation, e))?;

    let effective = if hooks.has_stack() {
        // Gate-mode stack: layers gate/rewrite/observe with full next()
        // mechanics; the terminal captures the effective input, which this
        // operation then executes.
        hooks
            .run_stack_gate(input_value, |_| Ok(()))
            .await
            .map_err(|e| format!("fs.{}: {}", operation, e))?
    } else {
        match hooks
            .run_pre(input_value)
            .await
            .map_err(|e| format!("fs.{}: hook chain error: {}", operation, e))?
        {
            PreOutcome::Allow(v) => v,
            PreOutcome::Deny(deny) => {
                return Err(format!(
                    "fs.{} {}: {} is not allowed",
                    operation, deny, path
                ));
            }
        }
    };

    super::hooks::verify_operation(&effective, operation, &format!("fs.{}", operation))?;

    let eff: FsEffective = serde_json::from_value(effective)
        .map_err(|e| format!("fs.{}: invalid effective input after pre hooks: {}", operation, e))?;

    // Fail closed on a hook that drops the destination of a two-path
    // operation (rename/copyFile/symlink): silently falling back to the
    // original destination would ignore part of the mutation and mask hook
    // misconfiguration.
    if destination.is_some() && eff.destination.is_none() {
        return Err(format!(
            "fs.{}: pre hook removed 'destination' from the input",
            operation
        ));
    }

    Ok(eff)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fs_policy_input_serialization() {
        let input = FsPolicyInput {
            operation: "readFile".to_string(),
            path: "/tmp/test.txt".to_string(),
            destination: None,
            recursive: None,
            encoding: Some("utf8".to_string()),
            mcp_headers: None,
        };
        let json = serde_json::to_string(&input).unwrap();
        assert!(json.contains("\"operation\":\"readFile\""));
        assert!(json.contains("\"path\":\"/tmp/test.txt\""));
        assert!(json.contains("\"encoding\":\"utf8\""));
        assert!(!json.contains("destination"));
        assert!(!json.contains("recursive"));
        assert!(!json.contains("mcp_headers"));
    }

    #[test]
    fn test_fs_policy_input_with_mcp_headers() {
        let input = FsPolicyInput {
            operation: "readFile".to_string(),
            path: "/data/workspace/abc-123/file.txt".to_string(),
            destination: None,
            recursive: None,
            encoding: None,
            mcp_headers: Some(serde_json::json!({"session-id": "abc-123"})),
        };
        let json = serde_json::to_string(&input).unwrap();
        assert!(json.contains("\"mcp_headers\""));
        assert!(json.contains("abc-123"));
    }

    #[test]
    fn io_errors_classify_by_kind_and_keep_the_node_code_token() {
        let e = std::io::Error::from(std::io::ErrorKind::NotFound);
        let err = FsError::io("readFile", "/tmp/x", &e);
        assert_eq!(err.kind, FsErrorKind::NotFound);
        assert!(err.message.starts_with("fs.readFile: /tmp/x: ENOENT: "), "{}", err.message);
        let e = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        assert_eq!(FsError::io2("rename", "/a", "/b", &e).kind, FsErrorKind::PermissionDenied);
        assert_eq!(FsError::io("mkdir", "/a", &std::io::Error::other("x")).kind, FsErrorKind::Other);
    }

    #[test]
    fn overlay_errors_classify_by_leading_code_token() {
        let err = FsError::overlay("rm", "/work/dir", "ENOTEMPTY: /work/dir");
        assert_eq!(err.kind, FsErrorKind::NotEmpty);
        assert_eq!(err.message, "fs.rm: /work/dir: ENOTEMPTY: /work/dir");
        assert_eq!(FsError::overlay("readlink", "/f", "EINVAL: /f is not a symlink").kind, FsErrorKind::Other);
        assert_eq!(FsError::not_found("readFile", "/missing").kind, FsErrorKind::NotFound);
        assert_eq!(FsError::not_found("readFile", "/missing").message, "fs.readFile: /missing: ENOENT");
        // A path that merely contains a code-like word is not a code token.
        assert_eq!(message_kind("fs.rm: /home/ENOENTish/file: boom"), FsErrorKind::Other);
    }

    #[test]
    fn gate_errors_only_count_denials_as_permission_failures() {
        let denied = FsError::gate("fs.readFile denied by policy: /x is not allowed".into());
        assert_eq!(denied.kind, FsErrorKind::PermissionDenied);
        let hook = FsError::gate("fs.readFile denied by pre hook (quota): /x is not allowed".into());
        assert_eq!(hook.kind, FsErrorKind::PermissionDenied);
        let chain = FsError::gate("fs.readFile: hook chain error: timeout".into());
        assert_eq!(chain.kind, FsErrorKind::Other);
    }

    #[test]
    fn stat_json_keeps_the_guest_wire_shape() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f.txt");
        std::fs::write(&file, b"abc").unwrap();
        let stat = FsStat::from_metadata(&std::fs::metadata(&file).unwrap());
        assert!(stat.is_file && !stat.is_directory && !stat.is_symlink);
        assert_eq!(stat.size, 3);
        let json: serde_json::Value = serde_json::from_str(&stat.to_json()).unwrap();
        for key in [
            "size", "isFile", "isDirectory", "isSymlink", "readonly", "mode", "ino", "dev", "nlink",
            "uid", "gid", "mtimeMs", "atimeMs", "ctimeMs", "birthtimeMs",
        ] {
            assert!(json.get(key).is_some(), "missing {key}");
        }
        assert_eq!(json["size"], 3);
        assert_eq!(json["isFile"], true);
        let overlay = FsStat::from_mount(&super::super::fs_mount::Stat {
            mode: 0o644,
            size: 9,
            is_dir: false,
            symlink: None,
        });
        let json: serde_json::Value = serde_json::from_str(&overlay.to_json()).unwrap();
        assert_eq!(json["mode"], 0o100644);
        assert_eq!(json["mtimeMs"], serde_json::Value::Null);
    }

    #[test]
    fn test_fs_policy_input_with_destination() {
        let input = FsPolicyInput {
            operation: "rename".to_string(),
            path: "/tmp/old.txt".to_string(),
            destination: Some("/tmp/new.txt".to_string()),
            recursive: None,
            encoding: None,
            mcp_headers: None,
        };
        let json = serde_json::to_string(&input).unwrap();
        assert!(json.contains("\"destination\":\"/tmp/new.txt\""));
    }
}
