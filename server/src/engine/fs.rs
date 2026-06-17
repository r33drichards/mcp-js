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
//! const data = await fs.readFile("/tmp/data.txt");          // string (utf-8)
//! const data = await fs.readFile("/tmp/data.bin", "buffer"); // Uint8Array
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

/// Configuration for the fs module. Stored in deno_core's `OpState`.
#[derive(Clone, Debug)]
pub struct FsConfig {
    pub policy_chain: Arc<PolicyChain>,
    pub mcp_headers: Option<serde_json::Value>,
    /// When a per-session overlay mount is attached, controls what happens on an
    /// overlay miss: `false` (default) = overlay-only (the overlay is the whole
    /// fs view; a miss is ENOENT). `true` = overlayfs-style — fall through to the
    /// real filesystem as a read-only lower layer (still policy-gated), so
    /// bundled paths like `/opt/languages` resolve while `/work` stays per-session.
    pub passthrough: bool,
}

impl FsConfig {
    pub fn new(chain: Arc<PolicyChain>) -> Self {
        Self { policy_chain: chain, mcp_headers: None, passthrough: false }
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

// ── Async deno_core ops ──────────────────────────────────────────────────

/// Read a file as UTF-8 text.
#[op2(async)]
#[string]
async fn op_fs_read_file_text(
    state: Rc<RefCell<OpState>>,
    #[string] path: String,
) -> Result<String, JsErrorBox> {
    let config = extract_config(&state)?;
    let mount = extract_mount(&state);

    // Mount-backed reads must run on the current-thread isolate runtime: the
    // CAS overlay uses deno_unsync, which asserts a current-thread flavor.
    // tokio::spawn would move the work onto the multi-thread runtime and abort
    // the process. Only the real-filesystem path is offloaded via spawn.
    if let Some(m) = mount {
        check_policy(&config.policy_chain, "readFile", &path, None, None, Some("utf8"), config.mcp_headers.as_ref())
            .await
            .map_err(JsErrorBox::generic)?;
        if let Some(content) = m.0.lock().await.read_opt(Path::new(&path)).await
            .map_err(|e| JsErrorBox::generic(format!("fs.readFile: {}: {}", path, e)))?
        {
            return String::from_utf8(content)
                .map_err(|e| JsErrorBox::generic(format!("fs.readFile: invalid UTF-8 in {}: {}", path, e)));
        }
        // Overlay miss. With passthrough off (default) the overlay is the whole
        // fs view, so this is ENOENT. With passthrough on, fall through to the
        // real filesystem as a read-only lower layer (already policy-gated above)
        // so bundled paths like /opt/languages resolve while /work stays the
        // per-session overlay.
        if !config.passthrough {
            return Err(JsErrorBox::generic(format!("fs.readFile: {}: ENOENT", path)));
        }
        let content = std::fs::read(&path)
            .map_err(|e| JsErrorBox::generic(io_err("readFile", &path, &e)))?;
        return String::from_utf8(content)
            .map_err(|e| JsErrorBox::generic(format!("fs.readFile: invalid UTF-8 in {}: {}", path, e)));
    }

    tokio::spawn(async move {
        check_policy(&config.policy_chain, "readFile", &path, None, None, Some("utf8"), config.mcp_headers.as_ref()).await?;

        let content = tokio::fs::read(&path).await
            .map_err(|e| io_err("readFile", &path, &e))?;

        String::from_utf8(content)
            .map_err(|e| format!("fs.readFile: invalid UTF-8 in {}: {}", path, e))
    })
    .await
    .map_err(|e| JsErrorBox::generic(format!("fs task join error: {}", e)))?
    .map_err(|e: String| JsErrorBox::generic(e))
}

/// Read a file as raw bytes, returned as a Uint8Array to JavaScript.
#[op2(async)]
#[buffer]
async fn op_fs_read_file_buffer(
    state: Rc<RefCell<OpState>>,
    #[string] path: String,
) -> Result<Vec<u8>, JsErrorBox> {
    let config = extract_config(&state)?;
    let mount = extract_mount(&state);

    // Mount branch runs inline (current-thread isolate runtime; deno_unsync needs it).
    if let Some(m) = mount {
        check_policy(&config.policy_chain, "readFile", &path, None, None, Some("buffer"), config.mcp_headers.as_ref())
            .await
            .map_err(JsErrorBox::generic)?;
        if let Some(content) = m.0.lock().await.read_opt(Path::new(&path)).await
            .map_err(|e| JsErrorBox::generic(format!("fs.readFile: {}: {}", path, e)))?
        {
            return Ok(content);
        }
        // Overlay miss: ENOENT unless passthrough is on, in which case fall
        // through to the real filesystem (policy-gated above) so bundled
        // read-only paths (e.g. /opt/languages) resolve.
        if !config.passthrough {
            return Err(JsErrorBox::generic(format!("fs.readFile: {}: ENOENT", path)));
        }
        return std::fs::read(&path)
            .map_err(|e| JsErrorBox::generic(io_err("readFile", &path, &e)));
    }

    tokio::spawn(async move {
        check_policy(&config.policy_chain, "readFile", &path, None, None, Some("buffer"), config.mcp_headers.as_ref()).await?;

        tokio::fs::read(&path).await
            .map_err(|e| io_err("readFile", &path, &e))
    })
    .await
    .map_err(|e| JsErrorBox::generic(format!("fs task join error: {}", e)))?
    .map_err(|e: String| JsErrorBox::generic(e))
}

/// Write a file from a UTF-8 string.
#[op2(async)]
#[string]
async fn op_fs_write_file_text(
    state: Rc<RefCell<OpState>>,
    #[string] path: String,
    #[string] data: String,
) -> Result<String, JsErrorBox> {
    let config = extract_config(&state)?;
    let mount = extract_mount(&state);

    // Mount-backed writes must run on the current-thread isolate runtime (the
    // CAS overlay uses deno_unsync, which asserts a current-thread flavor).
    // tokio::spawn would move the work onto the multi-thread runtime and abort.
    if let Some(m) = mount {
        check_policy(&config.policy_chain, "writeFile", &path, None, None, None, config.mcp_headers.as_ref())
            .await
            .map_err(JsErrorBox::generic)?;
        m.0.lock().await.write(Path::new(&path), data.as_bytes()).await
            .map_err(|e| JsErrorBox::generic(format!("fs.writeFile: {}: {}", path, e)))?;
        return Ok("{}".to_string());
    }

    tokio::spawn(async move {
        check_policy(&config.policy_chain, "writeFile", &path, None, None, None, config.mcp_headers.as_ref()).await?;

        tokio::fs::write(&path, data.as_bytes()).await
            .map_err(|e| io_err("writeFile", &path, &e))?;

        Ok("{}".to_string())
    })
    .await
    .map_err(|e| JsErrorBox::generic(format!("fs task join error: {}", e)))?
    .map_err(|e: String| JsErrorBox::generic(e))
}

/// Write a file from raw bytes (Uint8Array from JavaScript).
#[op2(async)]
#[string]
async fn op_fs_write_file_buffer(
    state: Rc<RefCell<OpState>>,
    #[string] path: String,
    #[buffer(copy)] data: Vec<u8>,
) -> Result<String, JsErrorBox> {
    let config = extract_config(&state)?;
    let mount = extract_mount(&state);

    // Mount branch runs inline (current-thread isolate runtime; deno_unsync needs it).
    if let Some(m) = mount {
        check_policy(&config.policy_chain, "writeFile", &path, None, None, None, config.mcp_headers.as_ref())
            .await
            .map_err(JsErrorBox::generic)?;
        m.0.lock().await.write(Path::new(&path), &data).await
            .map_err(|e| JsErrorBox::generic(format!("fs.writeFile: {}: {}", path, e)))?;
        return Ok("{}".to_string());
    }

    tokio::spawn(async move {
        check_policy(&config.policy_chain, "writeFile", &path, None, None, None, config.mcp_headers.as_ref()).await?;

        tokio::fs::write(&path, &data).await
            .map_err(|e| io_err("writeFile", &path, &e))?;

        Ok("{}".to_string())
    })
    .await
    .map_err(|e| JsErrorBox::generic(format!("fs task join error: {}", e)))?
    .map_err(|e: String| JsErrorBox::generic(e))
}

/// Append to a file.
#[op2(async)]
#[string]
async fn op_fs_append_file(
    state: Rc<RefCell<OpState>>,
    #[string] path: String,
    #[string] data: String,
) -> Result<String, JsErrorBox> {
    let config = extract_config(&state)?;
    let mount = extract_mount(&state);

    // Mount branch runs inline (current-thread isolate runtime; deno_unsync needs it).
    if let Some(m) = mount {
        check_policy(&config.policy_chain, "appendFile", &path, None, None, None, config.mcp_headers.as_ref())
            .await
            .map_err(JsErrorBox::generic)?;
        let mut guard = m.0.lock().await;
        let mut existing = guard.read(Path::new(&path)).await.unwrap_or_default();
        existing.extend_from_slice(data.as_bytes());
        guard.write(Path::new(&path), &existing).await
            .map_err(|e| JsErrorBox::generic(format!("fs.appendFile: {}: {}", path, e)))?;
        return Ok("{}".to_string());
    }

    tokio::spawn(async move {
        check_policy(&config.policy_chain, "appendFile", &path, None, None, None, config.mcp_headers.as_ref()).await?;

        use tokio::io::AsyncWriteExt;
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await
            .map_err(|e| io_err("appendFile", &path, &e))?;

        file.write_all(data.as_bytes()).await
            .map_err(|e| io_err("appendFile", &path, &e))?;

        Ok("{}".to_string())
    })
    .await
    .map_err(|e| JsErrorBox::generic(format!("fs task join error: {}", e)))?
    .map_err(|e: String| JsErrorBox::generic(e))
}

/// Read a directory. Returns JSON array of entry names.
#[op2(async)]
#[string]
async fn op_fs_readdir(
    state: Rc<RefCell<OpState>>,
    #[string] path: String,
) -> Result<String, JsErrorBox> {
    let config = extract_config(&state)?;
    let mount = extract_mount(&state);

    // Mount branch runs inline (current-thread isolate runtime; deno_unsync needs it).
    if let Some(m) = mount {
        check_policy(&config.policy_chain, "readdir", &path, None, None, None, config.mcp_headers.as_ref())
            .await
            .map_err(JsErrorBox::generic)?;
        let names = m.0.lock().await.readdir(Path::new(&path)).await
            .map_err(|e| JsErrorBox::generic(format!("fs.readdir: {}: {}", path, e)))?;
        return Ok(deno_core::serde_json::json!(names).to_string());
    }

    tokio::spawn(async move {
        check_policy(&config.policy_chain, "readdir", &path, None, None, None, config.mcp_headers.as_ref()).await?;

        let mut entries = Vec::new();
        let mut dir = tokio::fs::read_dir(&path).await
            .map_err(|e| io_err("readdir", &path, &e))?;

        while let Some(entry) = dir.next_entry().await
            .map_err(|e| io_err("readdir", &path, &e))? {
            if let Some(name) = entry.file_name().to_str() {
                entries.push(name.to_string());
            }
        }

        let result = deno_core::serde_json::json!(entries);
        Ok(result.to_string())
    })
    .await
    .map_err(|e| JsErrorBox::generic(format!("fs task join error: {}", e)))?
    .map_err(|e: String| JsErrorBox::generic(e))
}

/// Stat a path. Returns JSON with size, isFile, isDirectory, etc.
#[op2(async)]
#[string]
async fn op_fs_stat(
    state: Rc<RefCell<OpState>>,
    #[string] path: String,
) -> Result<String, JsErrorBox> {
    let config = extract_config(&state)?;
    let mount = extract_mount(&state);

    // Mount branch runs inline (current-thread isolate runtime; deno_unsync needs it).
    if let Some(m) = mount {
        check_policy(&config.policy_chain, "stat", &path, None, None, None, config.mcp_headers.as_ref())
            .await
            .map_err(JsErrorBox::generic)?;
        let s = m.0.lock().await.stat(Path::new(&path)).await
            .map_err(|e| JsErrorBox::generic(format!("fs.stat: {}: {}", path, e)))?;
        return Ok(mount_stat_json(&s));
    }

    tokio::spawn(async move {
        check_policy(&config.policy_chain, "stat", &path, None, None, None, config.mcp_headers.as_ref()).await?;

        let metadata = tokio::fs::metadata(&path).await
            .map_err(|e| io_err("stat", &path, &e))?;

        Ok(metadata_stat_json(&metadata))
    })
    .await
    .map_err(|e| JsErrorBox::generic(format!("fs task join error: {}", e)))?
    .map_err(|e: String| JsErrorBox::generic(e))
}

/// Stat a path **without** following a final symlink (Node `fs.lstat`).
#[op2(async)]
#[string]
async fn op_fs_lstat(
    state: Rc<RefCell<OpState>>,
    #[string] path: String,
) -> Result<String, JsErrorBox> {
    let config = extract_config(&state)?;
    let mount = extract_mount(&state);

    // Mount branch runs inline (current-thread isolate runtime; deno_unsync needs it).
    // The overlay never follows symlinks, so its stat already has lstat semantics.
    if let Some(m) = mount {
        check_policy(&config.policy_chain, "lstat", &path, None, None, None, config.mcp_headers.as_ref())
            .await
            .map_err(JsErrorBox::generic)?;
        let s = m.0.lock().await.stat(Path::new(&path)).await
            .map_err(|e| JsErrorBox::generic(format!("fs.lstat: {}: {}", path, e)))?;
        return Ok(mount_stat_json(&s));
    }

    tokio::spawn(async move {
        check_policy(&config.policy_chain, "lstat", &path, None, None, None, config.mcp_headers.as_ref()).await?;

        let metadata = tokio::fs::symlink_metadata(&path).await
            .map_err(|e| io_err("lstat", &path, &e))?;

        Ok(metadata_stat_json(&metadata))
    })
    .await
    .map_err(|e| JsErrorBox::generic(format!("fs task join error: {}", e)))?
    .map_err(|e: String| JsErrorBox::generic(e))
}

/// Read a symlink's target, returned as a string.
#[op2(async)]
#[string]
async fn op_fs_readlink(
    state: Rc<RefCell<OpState>>,
    #[string] path: String,
) -> Result<String, JsErrorBox> {
    let config = extract_config(&state)?;
    let mount = extract_mount(&state);

    // Mount branch runs inline (current-thread isolate runtime; deno_unsync needs it).
    if let Some(m) = mount {
        check_policy(&config.policy_chain, "readlink", &path, None, None, None, config.mcp_headers.as_ref())
            .await
            .map_err(JsErrorBox::generic)?;
        let target = m.0.lock().await.readlink(Path::new(&path)).await
            .map_err(|e| JsErrorBox::generic(format!("fs.readlink: {}: {}", path, e)))?;
        return Ok(target.to_string_lossy().into_owned());
    }

    tokio::spawn(async move {
        check_policy(&config.policy_chain, "readlink", &path, None, None, None, config.mcp_headers.as_ref()).await?;

        let target = tokio::fs::read_link(&path).await
            .map_err(|e| io_err("readlink", &path, &e))?;

        Ok(target.to_string_lossy().into_owned())
    })
    .await
    .map_err(|e| JsErrorBox::generic(format!("fs task join error: {}", e)))?
    .map_err(|e: String| JsErrorBox::generic(e))
}

/// Create a symlink at `link` pointing to `target` (Node `fs.symlink(target, path)`).
#[op2(async)]
#[string]
async fn op_fs_symlink(
    state: Rc<RefCell<OpState>>,
    #[string] target: String,
    #[string] link: String,
) -> Result<String, JsErrorBox> {
    let config = extract_config(&state)?;
    let mount = extract_mount(&state);

    // The policy gates on the link path being created; the target is carried as
    // the destination so a policy can constrain both sides.
    if let Some(m) = mount {
        check_policy(&config.policy_chain, "symlink", &link, Some(&target), None, None, config.mcp_headers.as_ref())
            .await
            .map_err(JsErrorBox::generic)?;
        m.0.lock().await.symlink(Path::new(&target), Path::new(&link)).await
            .map_err(|e| JsErrorBox::generic(format!("fs.symlink: {} -> {}: {}", link, target, e)))?;
        return Ok("{}".to_string());
    }

    tokio::spawn(async move {
        check_policy(&config.policy_chain, "symlink", &link, Some(&target), None, None, config.mcp_headers.as_ref()).await?;

        symlink_impl(&target, &link).await
            .map_err(|e| io_err2("symlink", &link, &target, &e))?;

        Ok("{}".to_string())
    })
    .await
    .map_err(|e| JsErrorBox::generic(format!("fs task join error: {}", e)))?
    .map_err(|e: String| JsErrorBox::generic(e))
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

/// Create a directory.
#[op2(async)]
#[string]
async fn op_fs_mkdir(
    state: Rc<RefCell<OpState>>,
    #[string] path: String,
    #[smi] recursive: i32,
) -> Result<String, JsErrorBox> {
    let config = extract_config(&state)?;
    let mount = extract_mount(&state);
    let recursive = recursive != 0;

    // Mount branch runs inline (current-thread isolate runtime; deno_unsync needs it).
    if let Some(m) = mount {
        check_policy(&config.policy_chain, "mkdir", &path, None, Some(recursive), None, config.mcp_headers.as_ref())
            .await
            .map_err(JsErrorBox::generic)?;
        m.0.lock().await.mkdir(Path::new(&path)).await
            .map_err(|e| JsErrorBox::generic(format!("fs.mkdir: {}: {}", path, e)))?;
        return Ok("{}".to_string());
    }

    tokio::spawn(async move {
        check_policy(&config.policy_chain, "mkdir", &path, None, Some(recursive), None, config.mcp_headers.as_ref()).await?;

        if recursive {
            tokio::fs::create_dir_all(&path).await
        } else {
            tokio::fs::create_dir(&path).await
        }.map_err(|e| io_err("mkdir", &path, &e))?;

        Ok("{}".to_string())
    })
    .await
    .map_err(|e| JsErrorBox::generic(format!("fs task join error: {}", e)))?
    .map_err(|e: String| JsErrorBox::generic(e))
}

/// Remove a file or directory.
#[op2(async)]
#[string]
async fn op_fs_rm(
    state: Rc<RefCell<OpState>>,
    #[string] path: String,
    #[smi] recursive: i32,
) -> Result<String, JsErrorBox> {
    let config = extract_config(&state)?;
    let mount = extract_mount(&state);
    let recursive = recursive != 0;

    // Mount branch runs inline (current-thread isolate runtime; deno_unsync needs it).
    if let Some(m) = mount {
        check_policy(&config.policy_chain, "rm", &path, None, Some(recursive), None, config.mcp_headers.as_ref())
            .await
            .map_err(JsErrorBox::generic)?;
        m.0.lock().await.remove(Path::new(&path), recursive).await
            .map_err(|e| JsErrorBox::generic(format!("fs.rm: {}: {}", path, e)))?;
        return Ok("{}".to_string());
    }

    tokio::spawn(async move {
        check_policy(&config.policy_chain, "rm", &path, None, Some(recursive), None, config.mcp_headers.as_ref()).await?;

        let metadata = tokio::fs::metadata(&path).await
            .map_err(|e| io_err("rm", &path, &e))?;

        if metadata.is_dir() {
            if recursive {
                tokio::fs::remove_dir_all(&path).await
            } else {
                tokio::fs::remove_dir(&path).await
            }
        } else {
            tokio::fs::remove_file(&path).await
        }.map_err(|e| io_err("rm", &path, &e))?;

        Ok("{}".to_string())
    })
    .await
    .map_err(|e| JsErrorBox::generic(format!("fs task join error: {}", e)))?
    .map_err(|e: String| JsErrorBox::generic(e))
}

/// Rename (move) a file or directory.
#[op2(async)]
#[string]
async fn op_fs_rename(
    state: Rc<RefCell<OpState>>,
    #[string] from: String,
    #[string] to: String,
) -> Result<String, JsErrorBox> {
    let config = extract_config(&state)?;
    let mount = extract_mount(&state);

    // Mount branch runs inline (current-thread isolate runtime; deno_unsync needs it).
    if let Some(m) = mount {
        check_policy(&config.policy_chain, "rename", &from, Some(&to), None, None, config.mcp_headers.as_ref())
            .await
            .map_err(JsErrorBox::generic)?;
        m.0.lock().await.rename(Path::new(&from), Path::new(&to)).await
            .map_err(|e| JsErrorBox::generic(format!("fs.rename: {} -> {}: {}", from, to, e)))?;
        return Ok("{}".to_string());
    }

    tokio::spawn(async move {
        check_policy(&config.policy_chain, "rename", &from, Some(&to), None, None, config.mcp_headers.as_ref()).await?;

        tokio::fs::rename(&from, &to).await
            .map_err(|e| io_err2("rename", &from, &to, &e))?;

        Ok("{}".to_string())
    })
    .await
    .map_err(|e| JsErrorBox::generic(format!("fs task join error: {}", e)))?
    .map_err(|e: String| JsErrorBox::generic(e))
}

/// Copy a file.
#[op2(async)]
#[string]
async fn op_fs_copy_file(
    state: Rc<RefCell<OpState>>,
    #[string] from: String,
    #[string] to: String,
) -> Result<String, JsErrorBox> {
    let config = extract_config(&state)?;
    let mount = extract_mount(&state);

    // Mount branch runs inline (current-thread isolate runtime; deno_unsync needs it).
    if let Some(m) = mount {
        check_policy(&config.policy_chain, "copyFile", &from, Some(&to), None, None, config.mcp_headers.as_ref())
            .await
            .map_err(JsErrorBox::generic)?;
        // Copy by reference: clones the content-addressed entry, no rechunk.
        m.0.lock().await.copy(Path::new(&from), Path::new(&to)).await
            .map_err(|e| JsErrorBox::generic(format!("fs.copyFile: {} -> {}: {}", from, to, e)))?;
        return Ok("{}".to_string());
    }

    tokio::spawn(async move {
        check_policy(&config.policy_chain, "copyFile", &from, Some(&to), None, None, config.mcp_headers.as_ref()).await?;

        tokio::fs::copy(&from, &to).await
            .map_err(|e| io_err2("copyFile", &from, &to, &e))?;

        Ok("{}".to_string())
    })
    .await
    .map_err(|e| JsErrorBox::generic(format!("fs task join error: {}", e)))?
    .map_err(|e: String| JsErrorBox::generic(e))
}

/// Check if a path exists.
#[op2(async)]
#[string]
async fn op_fs_exists(
    state: Rc<RefCell<OpState>>,
    #[string] path: String,
) -> Result<String, JsErrorBox> {
    let config = extract_config(&state)?;
    let mount = extract_mount(&state);

    // Mount branch runs inline (current-thread isolate runtime; deno_unsync needs it).
    if let Some(m) = mount {
        check_policy(&config.policy_chain, "exists", &path, None, None, None, config.mcp_headers.as_ref())
            .await
            .map_err(JsErrorBox::generic)?;
        let exists = m.0.lock().await.exists(Path::new(&path)).await;
        return Ok(if exists { "true" } else { "false" }.to_string());
    }

    tokio::spawn(async move {
        check_policy(&config.policy_chain, "exists", &path, None, None, None, config.mcp_headers.as_ref()).await?;

        let exists = tokio::fs::try_exists(&path).await.unwrap_or(false);
        Ok(if exists { "true" } else { "false" }.to_string())
    })
    .await
    .map_err(|e| JsErrorBox::generic(format!("fs task join error: {}", e)))?
    .map_err(|e: String| JsErrorBox::generic(e))
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
        check_policy(&config.policy_chain, "writeFile", &path, None, None, None, config.mcp_headers.as_ref())
            .await
            .map_err(JsErrorBox::generic)?;
        let store = m.0.lock().await.store_handle();
        let ow = OpenWrite::Overlay { path: path.clone(), writer: FileWriter::new(store) };
        let mut g = writers.0.lock().await;
        let id = g.next;
        g.next = g.next.wrapping_add(1);
        g.map.insert(id, ow);
        return Ok(id);
    }

    tokio::spawn(async move {
        check_policy(&config.policy_chain, "writeFile", &path, None, None, None, config.mcp_headers.as_ref()).await?;

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

    // Top-level fs.readFile keeps mcp-v8's historical contract: UTF-8 text by
    // default, raw bytes only when the encoding is the literal "buffer".
    async function readFile(path, encoding) {
        if (encoding === 'buffer') return await readFileBuffer(path);
        return await readFileText(path);
    }

    // Node's fs.promises.readFile contract: a Uint8Array by default, a string
    // when a text encoding is supplied. This is what Node-style consumers (e.g.
    // isomorphic-git reading binary git objects) expect.
    async function readFilePromises(path, options) {
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
        readFile: readFilePromises,
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

/// Build the JSON stat blob fs.stat / fs.lstat return, from host metadata. The
/// JS wrapper turns this into a Node `fs.Stats`-like object (with `isFile()` etc.).
fn metadata_stat_json(metadata: &std::fs::Metadata) -> String {
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

    deno_core::serde_json::json!({
        "size": metadata.len(),
        "isFile": metadata.is_file(),
        "isDirectory": metadata.is_dir(),
        "isSymlink": metadata.file_type().is_symlink(),
        "readonly": metadata.permissions().readonly(),
        "mode": mode,
        "ino": ino,
        "dev": dev,
        "nlink": nlink,
        "uid": uid,
        "gid": gid,
        "mtimeMs": modified,
        "atimeMs": accessed,
        "ctimeMs": ctime_ms,
        "birthtimeMs": created,
    })
    .to_string()
}

/// Build the JSON stat blob fs.stat returns, from overlay metadata.
fn mount_stat_json(s: &super::fs_mount::Stat) -> String {
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
    deno_core::serde_json::json!({
        "size": s.size,
        "isFile": !s.is_dir && !is_symlink,
        "isDirectory": s.is_dir,
        "isSymlink": is_symlink,
        "readonly": false,
        "mode": mode,
        "ino": 0,
        "dev": 0,
        "nlink": 1,
        "uid": 0,
        "gid": 0,
        "mtimeMs": null,
        "atimeMs": null,
        "ctimeMs": null,
        "birthtimeMs": null,
    })
    .to_string()
}

async fn check_policy(
    policy_chain: &PolicyChain,
    operation: &str,
    path: &str,
    destination: Option<&str>,
    recursive: Option<bool>,
    encoding: Option<&str>,
    mcp_headers: Option<&serde_json::Value>,
) -> Result<(), String> {
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

    let allowed = policy_chain
        .evaluate(&input_value)
        .await
        .map_err(|e| format!("fs.{}: policy error: {}", operation, e))?;

    if !allowed {
        return Err(format!(
            "fs.{} denied by policy: {} is not allowed",
            operation, path
        ));
    }

    Ok(())
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
