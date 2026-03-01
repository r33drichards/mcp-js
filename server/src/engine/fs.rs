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
//! const info = await fs.stat("/tmp/data.txt");               // {size,isFile,isDirectory,...}
//! await fs.mkdir("/tmp/newdir", { recursive: true });
//! await fs.rm("/tmp/data.txt");
//! await fs.rm("/tmp/newdir", { recursive: true });
//! await fs.rename("/tmp/old.txt", "/tmp/new.txt");
//! await fs.copyFile("/tmp/a.txt", "/tmp/b.txt");
//! const bool = await fs.exists("/tmp/data.txt");
//! ```

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use deno_core::{JsRuntime, OpState, op2};
use deno_error::JsErrorBox;
use serde::Serialize;

use super::opa::PolicyChain;

// ── Configuration ────────────────────────────────────────────────────────

/// Configuration for the fs module. Stored in deno_core's `OpState`.
#[derive(Clone, Debug)]
pub struct FsConfig {
    pub policy_chain: Arc<PolicyChain>,
}

impl FsConfig {
    pub fn new(chain: Arc<PolicyChain>) -> Self {
        Self { policy_chain: chain }
    }
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
}

// ── Async deno_core ops ──────────────────────────────────────────────────

/// Read a file as UTF-8 text.
#[op2(async)]
#[string]
async fn op_fs_read_file_text(
    state: Rc<RefCell<OpState>>,
    #[string] path: String,
) -> Result<String, JsErrorBox> {
    let policy_chain = extract_chain(&state)?;

    tokio::spawn(async move {
        check_policy(&policy_chain, "readFile", &path, None, None, Some("utf8")).await?;

        let content = tokio::fs::read(&path).await
            .map_err(|e| format!("fs.readFile: {}: {}", path, e))?;

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
    let policy_chain = extract_chain(&state)?;

    tokio::spawn(async move {
        check_policy(&policy_chain, "readFile", &path, None, None, Some("buffer")).await?;

        tokio::fs::read(&path).await
            .map_err(|e| format!("fs.readFile: {}: {}", path, e))
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
    let policy_chain = extract_chain(&state)?;

    tokio::spawn(async move {
        check_policy(&policy_chain, "writeFile", &path, None, None, None).await?;

        tokio::fs::write(&path, data.as_bytes()).await
            .map_err(|e| format!("fs.writeFile: {}: {}", path, e))?;

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
    let policy_chain = extract_chain(&state)?;

    tokio::spawn(async move {
        check_policy(&policy_chain, "writeFile", &path, None, None, None).await?;

        tokio::fs::write(&path, &data).await
            .map_err(|e| format!("fs.writeFile: {}: {}", path, e))?;

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
    let policy_chain = extract_chain(&state)?;

    tokio::spawn(async move {
        check_policy(&policy_chain, "appendFile", &path, None, None, None).await?;

        use tokio::io::AsyncWriteExt;
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await
            .map_err(|e| format!("fs.appendFile: {}: {}", path, e))?;

        file.write_all(data.as_bytes()).await
            .map_err(|e| format!("fs.appendFile: {}: {}", path, e))?;

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
    let policy_chain = extract_chain(&state)?;

    tokio::spawn(async move {
        check_policy(&policy_chain, "readdir", &path, None, None, None).await?;

        let mut entries = Vec::new();
        let mut dir = tokio::fs::read_dir(&path).await
            .map_err(|e| format!("fs.readdir: {}: {}", path, e))?;

        while let Some(entry) = dir.next_entry().await
            .map_err(|e| format!("fs.readdir: {}: {}", path, e))? {
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
    let policy_chain = extract_chain(&state)?;

    tokio::spawn(async move {
        check_policy(&policy_chain, "stat", &path, None, None, None).await?;

        let metadata = tokio::fs::metadata(&path).await
            .map_err(|e| format!("fs.stat: {}: {}", path, e))?;

        let modified = metadata.modified().ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as f64);
        let accessed = metadata.accessed().ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as f64);
        let created = metadata.created().ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as f64);

        let result = deno_core::serde_json::json!({
            "size": metadata.len(),
            "isFile": metadata.is_file(),
            "isDirectory": metadata.is_dir(),
            "isSymlink": metadata.file_type().is_symlink(),
            "readonly": metadata.permissions().readonly(),
            "mtimeMs": modified,
            "atimeMs": accessed,
            "birthtimeMs": created,
        });

        Ok(result.to_string())
    })
    .await
    .map_err(|e| JsErrorBox::generic(format!("fs task join error: {}", e)))?
    .map_err(|e: String| JsErrorBox::generic(e))
}

/// Create a directory.
#[op2(async)]
#[string]
async fn op_fs_mkdir(
    state: Rc<RefCell<OpState>>,
    #[string] path: String,
    #[smi] recursive: i32,
) -> Result<String, JsErrorBox> {
    let policy_chain = extract_chain(&state)?;
    let recursive = recursive != 0;

    tokio::spawn(async move {
        check_policy(&policy_chain, "mkdir", &path, None, Some(recursive), None).await?;

        if recursive {
            tokio::fs::create_dir_all(&path).await
        } else {
            tokio::fs::create_dir(&path).await
        }.map_err(|e| format!("fs.mkdir: {}: {}", path, e))?;

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
    let policy_chain = extract_chain(&state)?;
    let recursive = recursive != 0;

    tokio::spawn(async move {
        check_policy(&policy_chain, "rm", &path, None, Some(recursive), None).await?;

        let metadata = tokio::fs::metadata(&path).await
            .map_err(|e| format!("fs.rm: {}: {}", path, e))?;

        if metadata.is_dir() {
            if recursive {
                tokio::fs::remove_dir_all(&path).await
            } else {
                tokio::fs::remove_dir(&path).await
            }
        } else {
            tokio::fs::remove_file(&path).await
        }.map_err(|e| format!("fs.rm: {}: {}", path, e))?;

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
    let policy_chain = extract_chain(&state)?;

    tokio::spawn(async move {
        check_policy(&policy_chain, "rename", &from, Some(&to), None, None).await?;

        tokio::fs::rename(&from, &to).await
            .map_err(|e| format!("fs.rename: {} -> {}: {}", from, to, e))?;

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
    let policy_chain = extract_chain(&state)?;

    tokio::spawn(async move {
        check_policy(&policy_chain, "copyFile", &from, Some(&to), None, None).await?;

        tokio::fs::copy(&from, &to).await
            .map_err(|e| format!("fs.copyFile: {} -> {}: {}", from, to, e))?;

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
    let policy_chain = extract_chain(&state)?;

    tokio::spawn(async move {
        check_policy(&policy_chain, "exists", &path, None, None, None).await?;

        let exists = tokio::fs::try_exists(&path).await.unwrap_or(false);
        Ok(if exists { "true" } else { "false" }.to_string())
    })
    .await
    .map_err(|e| JsErrorBox::generic(format!("fs task join error: {}", e)))?
    .map_err(|e: String| JsErrorBox::generic(e))
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
        op_fs_mkdir,
        op_fs_rm,
        op_fs_rename,
        op_fs_copy_file,
        op_fs_exists,
    ],
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
    globalThis.fs = {
        readFile: async function(path, encoding) {
            if (typeof path !== 'string') throw new TypeError('fs.readFile: path must be a string');
            if (encoding === 'buffer') {
                return await Deno.core.ops.op_fs_read_file_buffer(path);
            }
            return await Deno.core.ops.op_fs_read_file_text(path);
        },

        writeFile: async function(path, data) {
            if (typeof path !== 'string') throw new TypeError('fs.writeFile: path must be a string');
            if (data instanceof Uint8Array) {
                await Deno.core.ops.op_fs_write_file_buffer(path, data);
            } else {
                await Deno.core.ops.op_fs_write_file_text(path, String(data));
            }
        },

        appendFile: async function(path, data) {
            if (typeof path !== 'string') throw new TypeError('fs.appendFile: path must be a string');
            await Deno.core.ops.op_fs_append_file(path, String(data));
        },

        readdir: async function(path) {
            if (typeof path !== 'string') throw new TypeError('fs.readdir: path must be a string');
            const raw = await Deno.core.ops.op_fs_readdir(path);
            return JSON.parse(raw);
        },

        stat: async function(path) {
            if (typeof path !== 'string') throw new TypeError('fs.stat: path must be a string');
            const raw = await Deno.core.ops.op_fs_stat(path);
            return JSON.parse(raw);
        },

        mkdir: async function(path, options) {
            if (typeof path !== 'string') throw new TypeError('fs.mkdir: path must be a string');
            const recursive = (options && options.recursive) ? 1 : 0;
            await Deno.core.ops.op_fs_mkdir(path, recursive);
        },

        rm: async function(path, options) {
            if (typeof path !== 'string') throw new TypeError('fs.rm: path must be a string');
            const recursive = (options && options.recursive) ? 1 : 0;
            await Deno.core.ops.op_fs_rm(path, recursive);
        },

        unlink: async function(path) {
            if (typeof path !== 'string') throw new TypeError('fs.unlink: path must be a string');
            await Deno.core.ops.op_fs_rm(path, 0);
        },

        rename: async function(oldPath, newPath) {
            if (typeof oldPath !== 'string') throw new TypeError('fs.rename: oldPath must be a string');
            if (typeof newPath !== 'string') throw new TypeError('fs.rename: newPath must be a string');
            await Deno.core.ops.op_fs_rename(oldPath, newPath);
        },

        copyFile: async function(src, dest) {
            if (typeof src !== 'string') throw new TypeError('fs.copyFile: src must be a string');
            if (typeof dest !== 'string') throw new TypeError('fs.copyFile: dest must be a string');
            await Deno.core.ops.op_fs_copy_file(src, dest);
        },

        exists: async function(path) {
            if (typeof path !== 'string') throw new TypeError('fs.exists: path must be a string');
            const raw = await Deno.core.ops.op_fs_exists(path);
            return raw === 'true';
        },
    };
})();
"#;

// ── Helpers ──────────────────────────────────────────────────────────────

fn extract_chain(state: &Rc<RefCell<OpState>>) -> Result<Arc<PolicyChain>, JsErrorBox> {
    let state = state.borrow();
    let config = state.try_borrow::<FsConfig>()
        .ok_or_else(|| JsErrorBox::generic("fs: internal error — no fs config available"))?;
    Ok(config.policy_chain.clone())
}

async fn check_policy(
    policy_chain: &PolicyChain,
    operation: &str,
    path: &str,
    destination: Option<&str>,
    recursive: Option<bool>,
    encoding: Option<&str>,
) -> Result<(), String> {
    let input = FsPolicyInput {
        operation: operation.to_string(),
        path: path.to_string(),
        destination: destination.map(|s| s.to_string()),
        recursive,
        encoding: encoding.map(|s| s.to_string()),
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
        };
        let json = serde_json::to_string(&input).unwrap();
        assert!(json.contains("\"operation\":\"readFile\""));
        assert!(json.contains("\"path\":\"/tmp/test.txt\""));
        assert!(json.contains("\"encoding\":\"utf8\""));
        assert!(!json.contains("destination"));
        assert!(!json.contains("recursive"));
    }

    #[test]
    fn test_fs_policy_input_with_destination() {
        let input = FsPolicyInput {
            operation: "rename".to_string(),
            path: "/tmp/old.txt".to_string(),
            destination: Some("/tmp/new.txt".to_string()),
            recursive: None,
            encoding: None,
        };
        let json = serde_json::to_string(&input).unwrap();
        assert!(json.contains("\"destination\":\"/tmp/new.txt\""));
    }
}
