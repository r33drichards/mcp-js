//! OPA-gated filesystem operations for the JavaScript runtime.
//!
//! Provides a Node.js-compatible `fs` API with every operation gated by an OPA
//! policy. Each call sends an input document describing the operation, path(s),
//! and any relevant metadata to the configured OPA endpoint, which must return
//! `{"allow": true}` for the operation to proceed.
//!
//! Available operations (all return Promises):
//! ```js
//! const data = await fs.readFile("/tmp/data.txt");          // string (utf-8)
//! const data = await fs.readFile("/tmp/data.bin", "buffer"); // Uint8Array
//! await fs.writeFile("/tmp/out.txt", "hello");
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

use deno_core::{JsRuntime, OpState, op2};
use deno_error::JsErrorBox;
use serde::Serialize;

use super::opa::OpaClient;

// ── Configuration ────────────────────────────────────────────────────────

/// Configuration for the fs module, including OPA policy settings.
/// Stored in deno_core's `OpState` for access from async ops.
#[derive(Clone, Debug)]
pub struct FsConfig {
    pub opa_client: OpaClient,
    pub opa_policy_path: String,
}

impl FsConfig {
    pub fn new(opa_url: String, opa_policy_path: String) -> Self {
        Self {
            opa_client: OpaClient::new(opa_url),
            opa_policy_path,
        }
    }
}

// ── OPA policy input ─────────────────────────────────────────────────────

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

/// Read a file. encoding: "utf8" (default, returns string) or "buffer" (returns base64).
#[op2(async)]
#[string]
async fn op_fs_read_file(
    state: Rc<RefCell<OpState>>,
    #[string] path: String,
    #[string] encoding: String,
) -> Result<String, JsErrorBox> {
    let (opa_client, opa_policy_path) = extract_config(&state)?;
    let encoding = if encoding.is_empty() { "utf8".to_string() } else { encoding };

    check_policy(&opa_client, &opa_policy_path, "readFile", &path, None, None, Some(&encoding)).await?;

    let content = tokio::fs::read(&path).await
        .map_err(|e| JsErrorBox::generic(format!("fs.readFile: {}: {}", path, e)))?;

    if encoding == "buffer" {
        // Return base64-encoded bytes for binary reading
        use deno_core::serde_json;
        let b64 = base64_encode(&content);
        let result = serde_json::json!({ "type": "buffer", "data": b64 });
        Ok(result.to_string())
    } else {
        let text = String::from_utf8(content)
            .map_err(|e| JsErrorBox::generic(format!("fs.readFile: invalid UTF-8 in {}: {}", path, e)))?;
        let result = deno_core::serde_json::json!({ "type": "utf8", "data": text });
        Ok(result.to_string())
    }
}

/// Write a file (creates or truncates). data_b64 is base64 if is_binary is true.
#[op2(async)]
#[string]
async fn op_fs_write_file(
    state: Rc<RefCell<OpState>>,
    #[string] path: String,
    #[string] data: String,
    #[smi] is_binary: i32,
) -> Result<String, JsErrorBox> {
    let (opa_client, opa_policy_path) = extract_config(&state)?;

    check_policy(&opa_client, &opa_policy_path, "writeFile", &path, None, None, None).await?;

    let bytes = if is_binary != 0 {
        base64_decode(&data)
            .map_err(|e| JsErrorBox::generic(format!("fs.writeFile: invalid base64: {}", e)))?
    } else {
        data.into_bytes()
    };

    tokio::fs::write(&path, &bytes).await
        .map_err(|e| JsErrorBox::generic(format!("fs.writeFile: {}: {}", path, e)))?;

    Ok("{}".to_string())
}

/// Append to a file.
#[op2(async)]
#[string]
async fn op_fs_append_file(
    state: Rc<RefCell<OpState>>,
    #[string] path: String,
    #[string] data: String,
) -> Result<String, JsErrorBox> {
    let (opa_client, opa_policy_path) = extract_config(&state)?;

    check_policy(&opa_client, &opa_policy_path, "appendFile", &path, None, None, None).await?;

    use tokio::io::AsyncWriteExt;
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .await
        .map_err(|e| JsErrorBox::generic(format!("fs.appendFile: {}: {}", path, e)))?;

    file.write_all(data.as_bytes()).await
        .map_err(|e| JsErrorBox::generic(format!("fs.appendFile: {}: {}", path, e)))?;

    Ok("{}".to_string())
}

/// Read a directory. Returns JSON array of entry names.
#[op2(async)]
#[string]
async fn op_fs_readdir(
    state: Rc<RefCell<OpState>>,
    #[string] path: String,
) -> Result<String, JsErrorBox> {
    let (opa_client, opa_policy_path) = extract_config(&state)?;

    check_policy(&opa_client, &opa_policy_path, "readdir", &path, None, None, None).await?;

    let mut entries = Vec::new();
    let mut dir = tokio::fs::read_dir(&path).await
        .map_err(|e| JsErrorBox::generic(format!("fs.readdir: {}: {}", path, e)))?;

    while let Some(entry) = dir.next_entry().await
        .map_err(|e| JsErrorBox::generic(format!("fs.readdir: {}: {}", path, e)))? {
        if let Some(name) = entry.file_name().to_str() {
            entries.push(name.to_string());
        }
    }

    let result = deno_core::serde_json::json!(entries);
    Ok(result.to_string())
}

/// Stat a path. Returns JSON with size, isFile, isDirectory, etc.
#[op2(async)]
#[string]
async fn op_fs_stat(
    state: Rc<RefCell<OpState>>,
    #[string] path: String,
) -> Result<String, JsErrorBox> {
    let (opa_client, opa_policy_path) = extract_config(&state)?;

    check_policy(&opa_client, &opa_policy_path, "stat", &path, None, None, None).await?;

    let metadata = tokio::fs::metadata(&path).await
        .map_err(|e| JsErrorBox::generic(format!("fs.stat: {}: {}", path, e)))?;

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
}

/// Create a directory. Supports recursive creation.
#[op2(async)]
#[string]
async fn op_fs_mkdir(
    state: Rc<RefCell<OpState>>,
    #[string] path: String,
    #[smi] recursive: i32,
) -> Result<String, JsErrorBox> {
    let (opa_client, opa_policy_path) = extract_config(&state)?;
    let recursive = recursive != 0;

    check_policy(&opa_client, &opa_policy_path, "mkdir", &path, None, Some(recursive), None).await?;

    if recursive {
        tokio::fs::create_dir_all(&path).await
    } else {
        tokio::fs::create_dir(&path).await
    }.map_err(|e| JsErrorBox::generic(format!("fs.mkdir: {}: {}", path, e)))?;

    Ok("{}".to_string())
}

/// Remove a file or directory. Supports recursive deletion.
#[op2(async)]
#[string]
async fn op_fs_rm(
    state: Rc<RefCell<OpState>>,
    #[string] path: String,
    #[smi] recursive: i32,
) -> Result<String, JsErrorBox> {
    let (opa_client, opa_policy_path) = extract_config(&state)?;
    let recursive = recursive != 0;

    check_policy(&opa_client, &opa_policy_path, "rm", &path, None, Some(recursive), None).await?;

    let metadata = tokio::fs::metadata(&path).await
        .map_err(|e| JsErrorBox::generic(format!("fs.rm: {}: {}", path, e)))?;

    if metadata.is_dir() {
        if recursive {
            tokio::fs::remove_dir_all(&path).await
        } else {
            tokio::fs::remove_dir(&path).await
        }
    } else {
        tokio::fs::remove_file(&path).await
    }.map_err(|e| JsErrorBox::generic(format!("fs.rm: {}: {}", path, e)))?;

    Ok("{}".to_string())
}

/// Rename (move) a file or directory.
#[op2(async)]
#[string]
async fn op_fs_rename(
    state: Rc<RefCell<OpState>>,
    #[string] from: String,
    #[string] to: String,
) -> Result<String, JsErrorBox> {
    let (opa_client, opa_policy_path) = extract_config(&state)?;

    check_policy(&opa_client, &opa_policy_path, "rename", &from, Some(&to), None, None).await?;

    tokio::fs::rename(&from, &to).await
        .map_err(|e| JsErrorBox::generic(format!("fs.rename: {} -> {}: {}", from, to, e)))?;

    Ok("{}".to_string())
}

/// Copy a file.
#[op2(async)]
#[string]
async fn op_fs_copy_file(
    state: Rc<RefCell<OpState>>,
    #[string] from: String,
    #[string] to: String,
) -> Result<String, JsErrorBox> {
    let (opa_client, opa_policy_path) = extract_config(&state)?;

    check_policy(&opa_client, &opa_policy_path, "copyFile", &from, Some(&to), None, None).await?;

    tokio::fs::copy(&from, &to).await
        .map_err(|e| JsErrorBox::generic(format!("fs.copyFile: {} -> {}: {}", from, to, e)))?;

    Ok("{}".to_string())
}

/// Check if a path exists. Does not follow symlinks.
#[op2(async)]
#[string]
async fn op_fs_exists(
    state: Rc<RefCell<OpState>>,
    #[string] path: String,
) -> Result<String, JsErrorBox> {
    let (opa_client, opa_policy_path) = extract_config(&state)?;

    check_policy(&opa_client, &opa_policy_path, "exists", &path, None, None, None).await?;

    let exists = tokio::fs::try_exists(&path).await.unwrap_or(false);
    Ok(if exists { "true" } else { "false" }.to_string())
}

// ── Extension registration ──────────────────────────────────────────────

deno_core::extension!(
    fs_ext,
    ops = [
        op_fs_read_file,
        op_fs_write_file,
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

/// Create the fs extension for use in `RuntimeOptions::extensions`.
pub fn create_extension() -> deno_core::Extension {
    fs_ext::init()
}

// ── Inject fs JS wrapper into the global scope ──────────────────────────

/// Inject the `globalThis.fs` JS wrapper. Must be called after the
/// runtime is created (with the fs extension) but before user code runs.
pub fn inject_fs(runtime: &mut JsRuntime) -> Result<(), String> {
    runtime
        .execute_script("<fs-setup>", FS_JS_WRAPPER.to_string())
        .map_err(|e| format!("Failed to install fs wrapper: {}", e))?;
    Ok(())
}

/// JavaScript wrapper that provides a Node.js-compatible `fs` API via async ops.
const FS_JS_WRAPPER: &str = r#"
(function() {
    // Base64 encoder/decoder for binary data
    const _b64chars = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';

    function _uint8ToBase64(bytes) {
        let result = '';
        const len = bytes.length;
        for (let i = 0; i < len; i += 3) {
            const a = bytes[i];
            const b = i + 1 < len ? bytes[i + 1] : 0;
            const c = i + 2 < len ? bytes[i + 2] : 0;
            result += _b64chars[a >> 2];
            result += _b64chars[((a & 3) << 4) | (b >> 4)];
            result += i + 1 < len ? _b64chars[((b & 15) << 2) | (c >> 6)] : '=';
            result += i + 2 < len ? _b64chars[c & 63] : '=';
        }
        return result;
    }

    function _base64ToUint8(b64) {
        const lookup = new Uint8Array(256);
        for (let i = 0; i < _b64chars.length; i++) lookup[_b64chars.charCodeAt(i)] = i;
        let bufLen = b64.length * 3 / 4;
        if (b64[b64.length - 1] === '=') bufLen--;
        if (b64[b64.length - 2] === '=') bufLen--;
        const bytes = new Uint8Array(bufLen);
        let p = 0;
        for (let i = 0; i < b64.length; i += 4) {
            const a = lookup[b64.charCodeAt(i)];
            const b = lookup[b64.charCodeAt(i + 1)];
            const c = lookup[b64.charCodeAt(i + 2)];
            const d = lookup[b64.charCodeAt(i + 3)];
            bytes[p++] = (a << 2) | (b >> 4);
            if (p < bufLen) bytes[p++] = ((b & 15) << 4) | (c >> 2);
            if (p < bufLen) bytes[p++] = ((c & 3) << 6) | d;
        }
        return bytes;
    }

    globalThis.fs = {
        readFile: async function(path, encoding) {
            if (typeof path !== 'string') throw new TypeError('fs.readFile: path must be a string');
            const enc = encoding || 'utf8';
            const raw = await Deno.core.ops.op_fs_read_file(path, enc);
            const result = JSON.parse(raw);
            if (result.type === 'buffer') {
                return _base64ToUint8(result.data);
            }
            return result.data;
        },

        writeFile: async function(path, data) {
            if (typeof path !== 'string') throw new TypeError('fs.writeFile: path must be a string');
            let isBinary = 0;
            let payload = '';
            if (data instanceof Uint8Array) {
                isBinary = 1;
                payload = _uint8ToBase64(data);
            } else {
                payload = String(data);
            }
            await Deno.core.ops.op_fs_write_file(path, payload, isBinary);
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

fn extract_config(state: &Rc<RefCell<OpState>>) -> Result<(OpaClient, String), JsErrorBox> {
    let state = state.borrow();
    let config = state.try_borrow::<FsConfig>()
        .ok_or_else(|| JsErrorBox::generic("fs: internal error — no fs config available"))?;
    Ok((config.opa_client.clone(), config.opa_policy_path.clone()))
}

async fn check_policy(
    opa_client: &OpaClient,
    opa_policy_path: &str,
    operation: &str,
    path: &str,
    destination: Option<&str>,
    recursive: Option<bool>,
    encoding: Option<&str>,
) -> Result<(), JsErrorBox> {
    let input = FsPolicyInput {
        operation: operation.to_string(),
        path: path.to_string(),
        destination: destination.map(|s| s.to_string()),
        recursive,
        encoding: encoding.map(|s| s.to_string()),
    };

    let allowed = opa_client
        .evaluate(opa_policy_path, &input)
        .await
        .map_err(|e| JsErrorBox::generic(format!("fs.{}: OPA error: {}", operation, e)))?;

    if !allowed {
        return Err(JsErrorBox::generic(format!(
            "fs.{} denied by policy: {} is not allowed",
            operation, path
        )));
    }

    Ok(())
}

fn base64_encode(data: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let a = chunk[0] as usize;
        let b = if chunk.len() > 1 { chunk[1] as usize } else { 0 };
        let c = if chunk.len() > 2 { chunk[2] as usize } else { 0 };
        result.push(CHARS[a >> 2] as char);
        result.push(CHARS[((a & 3) << 4) | (b >> 4)] as char);
        if chunk.len() > 1 {
            result.push(CHARS[((b & 15) << 2) | (c >> 6)] as char);
        } else {
            result.push('=');
        }
        if chunk.len() > 2 {
            result.push(CHARS[c & 63] as char);
        } else {
            result.push('=');
        }
    }
    result
}

fn base64_decode(input: &str) -> Result<Vec<u8>, String> {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut lookup = [255u8; 256];
    for (i, &c) in CHARS.iter().enumerate() {
        lookup[c as usize] = i as u8;
    }

    let bytes = input.as_bytes();
    let mut result = Vec::with_capacity(bytes.len() * 3 / 4);

    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'=' || bytes[i] == b'\n' || bytes[i] == b'\r' {
            i += 1;
            continue;
        }

        let a = lookup[bytes[i] as usize];
        let b = if i + 1 < bytes.len() { lookup[bytes[i + 1] as usize] } else { 0 };
        let c = if i + 2 < bytes.len() && bytes[i + 2] != b'=' { lookup[bytes[i + 2] as usize] } else { 0 };
        let d = if i + 3 < bytes.len() && bytes[i + 3] != b'=' { lookup[bytes[i + 3] as usize] } else { 0 };

        if a == 255 || b == 255 {
            return Err("Invalid base64 character".to_string());
        }

        result.push((a << 2) | (b >> 4));
        if i + 2 < bytes.len() && bytes[i + 2] != b'=' {
            result.push(((b & 15) << 4) | (c >> 2));
        }
        if i + 3 < bytes.len() && bytes[i + 3] != b'=' {
            result.push(((c & 3) << 6) | d);
        }

        i += 4;
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_base64_roundtrip() {
        let original = b"Hello, World! \x00\x01\x02\xff";
        let encoded = base64_encode(original);
        let decoded = base64_decode(&encoded).unwrap();
        assert_eq!(original.as_slice(), decoded.as_slice());
    }

    #[test]
    fn test_base64_empty() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_decode("").unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn test_base64_padding() {
        assert_eq!(base64_encode(b"a"), "YQ==");
        assert_eq!(base64_encode(b"ab"), "YWI=");
        assert_eq!(base64_encode(b"abc"), "YWJj");
    }

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
        // destination and recursive should be skipped
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
