//! OPA-gated subprocess execution for the JavaScript runtime.
//!
//! Provides two subprocess APIs, both policy-gated via OPA:
//!
//! **Deno.Command** (Deno subprocess API):
//! ```js
//! const cmd = new Deno.Command("echo", { args: ["hello"] });
//! const { code, stdout, stderr } = await cmd.output();
//! console.log(new TextDecoder().decode(stdout));
//! ```
//!
//! **child_process.exec** (Node.js-compatible):
//! ```js
//! const { stdout, stderr } = await child_process.exec("echo hello");
//! const result = await child_process.exec("ls -la", { cwd: "/tmp" });
//! ```
//!
//! Every subprocess invocation is evaluated against an OPA policy chain
//! before execution. The policy input includes the command, arguments,
//! working directory, and environment variables.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use deno_core::{JsRuntime, OpState, op2};
use deno_error::JsErrorBox;
use serde::Serialize;

use super::opa::PolicyChain;

// ── Configuration ────────────────────────────────────────────────────────

/// Configuration for subprocess execution. Stored in deno_core's `OpState`.
#[derive(Clone, Debug)]
pub struct SubprocessConfig {
    pub policy_chain: Arc<PolicyChain>,
}

impl SubprocessConfig {
    pub fn new(chain: Arc<PolicyChain>) -> Self {
        Self { policy_chain: chain }
    }
}

// ── Policy input ─────────────────────────────────────────────────────────

#[derive(Serialize)]
struct SubprocessPolicyInput {
    /// The type of operation: "command_output", "command_spawn", or "exec".
    operation: String,
    /// The command to execute.
    command: String,
    /// Arguments to the command.
    args: Vec<String>,
    /// Working directory (if specified).
    #[serde(skip_serializing_if = "Option::is_none")]
    cwd: Option<String>,
    /// Environment variables (if specified).
    #[serde(skip_serializing_if = "Option::is_none")]
    env: Option<HashMap<String, String>>,
}

// ── Async deno_core ops ──────────────────────────────────────────────────

/// Async op: Run a command to completion (Deno.Command.output() equivalent).
/// Called from JS via `Deno.core.ops.op_subprocess_output(command, args_json, options_json)`.
/// Returns a JSON string with {code, stdout, stderr}.
#[op2(async)]
#[string]
async fn op_subprocess_output(
    state: Rc<RefCell<OpState>>,
    #[string] command: String,
    #[string] args_json: String,
    #[string] options_json: String,
) -> Result<String, JsErrorBox> {
    let policy_chain = extract_chain(&state)?;

    tokio::spawn(async move {
        let args: Vec<String> = serde_json::from_str(&args_json)
            .map_err(|e| format!("subprocess: invalid args JSON: {}", e))?;
        let options: SubprocessOptions = serde_json::from_str(&options_json)
            .map_err(|e| format!("subprocess: invalid options JSON: {}", e))?;

        check_policy(&policy_chain, "command_output", &command, &args, &options).await?;

        let mut cmd = tokio::process::Command::new(&command);
        cmd.args(&args);

        if let Some(ref cwd) = options.cwd {
            cmd.current_dir(cwd);
        }
        if let Some(ref env) = options.env {
            cmd.envs(env);
        }

        let output = cmd.output().await
            .map_err(|e| format!("subprocess: failed to execute '{}': {}", command, e))?;

        let stdout = base64_encode(&output.stdout);
        let stderr = base64_encode(&output.stderr);

        let result = serde_json::json!({
            "code": output.status.code().unwrap_or(-1),
            "stdout": stdout,
            "stderr": stderr,
            "success": output.status.success(),
        });

        Ok(result.to_string())
    })
    .await
    .map_err(|e| JsErrorBox::generic(format!("subprocess task join error: {}", e)))?
    .map_err(|e: String| JsErrorBox::generic(e))
}

/// Async op: Execute a shell command (Node.js child_process.exec equivalent).
/// Called from JS via `Deno.core.ops.op_subprocess_exec(command, options_json)`.
/// Returns a JSON string with {code, stdout, stderr}.
#[op2(async)]
#[string]
async fn op_subprocess_exec(
    state: Rc<RefCell<OpState>>,
    #[string] command: String,
    #[string] options_json: String,
) -> Result<String, JsErrorBox> {
    let policy_chain = extract_chain(&state)?;

    tokio::spawn(async move {
        let options: SubprocessOptions = serde_json::from_str(&options_json)
            .map_err(|e| format!("subprocess.exec: invalid options JSON: {}", e))?;

        // For exec(), the command is run via shell. We pass it as a single
        // string to the shell, so args in the policy input is the full command.
        let shell = if cfg!(target_os = "windows") { "cmd" } else { "/bin/sh" };
        let shell_arg = if cfg!(target_os = "windows") { "/C" } else { "-c" };

        check_policy(
            &policy_chain,
            "exec",
            shell,
            &[shell_arg.to_string(), command.clone()],
            &options,
        ).await?;

        let mut cmd = tokio::process::Command::new(shell);
        cmd.arg(shell_arg).arg(&command);

        if let Some(ref cwd) = options.cwd {
            cmd.current_dir(cwd);
        }
        if let Some(ref env) = options.env {
            cmd.envs(env);
        }

        let output = cmd.output().await
            .map_err(|e| format!("subprocess.exec: failed to execute '{}': {}", command, e))?;

        let encoding = options.encoding.as_deref().unwrap_or("utf8");
        let (stdout, stderr) = if encoding == "buffer" {
            (base64_encode(&output.stdout), base64_encode(&output.stderr))
        } else {
            (
                String::from_utf8_lossy(&output.stdout).to_string(),
                String::from_utf8_lossy(&output.stderr).to_string(),
            )
        };

        let result = serde_json::json!({
            "code": output.status.code().unwrap_or(-1),
            "stdout": stdout,
            "stderr": stderr,
            "success": output.status.success(),
            "encoding": encoding,
        });

        Ok(result.to_string())
    })
    .await
    .map_err(|e| JsErrorBox::generic(format!("subprocess task join error: {}", e)))?
    .map_err(|e: String| JsErrorBox::generic(e))
}

// ── Extension registration ──────────────────────────────────────────────

deno_core::extension!(
    subprocess_ext,
    ops = [op_subprocess_output, op_subprocess_exec],
);

pub fn create_extension() -> deno_core::Extension {
    subprocess_ext::init()
}

// ── Inject subprocess JS wrappers into the global scope ─────────────────

pub fn inject_subprocess(runtime: &mut JsRuntime) -> Result<(), String> {
    runtime
        .execute_script("<subprocess-setup>", SUBPROCESS_JS_WRAPPER.to_string())
        .map_err(|e| format!("Failed to install subprocess wrapper: {}", e))?;
    Ok(())
}

/// JavaScript wrapper providing both Deno.Command and child_process.exec APIs.
const SUBPROCESS_JS_WRAPPER: &str = r#"
(function() {
    // ── Base64 decode helper (for binary output from Deno.Command) ──────
    function base64ToUint8Array(base64) {
        var raw = '';
        var chars = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';
        // Remove padding
        var stripped = base64.replace(/=+$/, '');
        for (var i = 0; i < stripped.length; i += 4) {
            var a = chars.indexOf(stripped[i]);
            var b = chars.indexOf(stripped[i + 1]);
            var c = chars.indexOf(stripped[i + 2]);
            var d = chars.indexOf(stripped[i + 3]);
            var n = (a << 18) | (b << 12) | ((c >= 0 ? c : 0) << 6) | (d >= 0 ? d : 0);
            raw += String.fromCharCode((n >> 16) & 0xFF);
            if (c >= 0) raw += String.fromCharCode((n >> 8) & 0xFF);
            if (d >= 0) raw += String.fromCharCode(n & 0xFF);
        }
        var bytes = new Uint8Array(raw.length);
        for (var j = 0; j < raw.length; j++) {
            bytes[j] = raw.charCodeAt(j);
        }
        return bytes;
    }

    // ── Deno.Command API ────────────────────────────────────────────────
    // https://docs.deno.com/api/deno/subprocess
    //
    // Usage:
    //   const cmd = new Deno.Command("echo", { args: ["hello"] });
    //   const { code, stdout, stderr, success } = await cmd.output();
    //
    //   // stdout/stderr are Uint8Array by default
    //   console.log(new TextDecoder().decode(stdout));

    function DenoCommand(command, options) {
        if (typeof command !== 'string') {
            throw new TypeError('Deno.Command: command must be a string');
        }
        this._command = command;
        this._options = options || {};
    }

    DenoCommand.prototype.output = async function() {
        var args = this._options.args || [];
        if (!Array.isArray(args)) {
            throw new TypeError('Deno.Command: args must be an array');
        }
        var argsJson = JSON.stringify(args.map(String));
        var optionsJson = JSON.stringify({
            cwd: this._options.cwd || null,
            env: this._options.env || null,
        });

        var rawResult = await Deno.core.ops.op_subprocess_output(
            this._command, argsJson, optionsJson
        );
        var result = JSON.parse(rawResult);

        return {
            code: result.code,
            success: result.success,
            stdout: base64ToUint8Array(result.stdout),
            stderr: base64ToUint8Array(result.stderr),
            signal: null,
        };
    };

    DenoCommand.prototype.outputSync = function() {
        throw new Error('Deno.Command.outputSync() is not supported; use output() instead');
    };

    DenoCommand.prototype.spawn = function() {
        throw new Error('Deno.Command.spawn() is not yet supported; use output() instead');
    };

    // Attach to Deno namespace (create if needed)
    if (typeof globalThis.Deno === 'undefined') {
        globalThis.Deno = {};
    }
    globalThis.Deno.Command = DenoCommand;

    // ── child_process.exec API ──────────────────────────────────────────
    // https://docs.deno.com/api/node/child_process/~/exec
    //
    // Usage:
    //   const { stdout, stderr } = await child_process.exec("echo hello");
    //   const result = await child_process.exec("ls -la", { cwd: "/tmp" });
    //
    // Returns a Promise that resolves to { code, stdout, stderr, success }.
    // Encoding defaults to "utf8" (returns strings).
    // Set encoding to "buffer" to get base64-encoded binary output.

    globalThis.child_process = {
        exec: async function(command, options) {
            if (typeof command !== 'string') {
                throw new TypeError('child_process.exec: command must be a string');
            }
            var opts = options || {};
            var optionsJson = JSON.stringify({
                cwd: opts.cwd || null,
                env: opts.env || null,
                encoding: opts.encoding || 'utf8',
                timeout: opts.timeout || null,
            });

            var rawResult = await Deno.core.ops.op_subprocess_exec(
                command, optionsJson
            );
            var result = JSON.parse(rawResult);

            if (result.encoding === 'buffer') {
                result.stdout = base64ToUint8Array(result.stdout);
                result.stderr = base64ToUint8Array(result.stderr);
            }

            return {
                code: result.code,
                stdout: result.stdout,
                stderr: result.stderr,
                success: result.success,
            };
        },
    };
})();
"#;

// ── Helpers ──────────────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
struct SubprocessOptions {
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    env: Option<HashMap<String, String>>,
    #[serde(default)]
    encoding: Option<String>,
    #[allow(dead_code)]
    #[serde(default)]
    timeout: Option<u64>,
}

fn extract_chain(state: &Rc<RefCell<OpState>>) -> Result<Arc<PolicyChain>, JsErrorBox> {
    let state = state.borrow();
    let config = state.try_borrow::<SubprocessConfig>()
        .ok_or_else(|| JsErrorBox::generic("subprocess: internal error — no subprocess config available"))?;
    Ok(config.policy_chain.clone())
}

async fn check_policy(
    policy_chain: &PolicyChain,
    operation: &str,
    command: &str,
    args: &[String],
    options: &SubprocessOptions,
) -> Result<(), String> {
    let input = SubprocessPolicyInput {
        operation: operation.to_string(),
        command: command.to_string(),
        args: args.to_vec(),
        cwd: options.cwd.clone(),
        env: options.env.clone(),
    };

    let input_value = serde_json::to_value(&input)
        .map_err(|e| format!("subprocess.{}: failed to serialize policy input: {}", operation, e))?;

    let allowed = policy_chain
        .evaluate(&input_value)
        .await
        .map_err(|e| format!("subprocess.{}: policy error: {}", operation, e))?;

    if !allowed {
        return Err(format!(
            "subprocess.{} denied by policy: '{}' is not allowed",
            operation, command
        ));
    }

    Ok(())
}

fn base64_encode(data: &[u8]) -> String {
    use std::fmt::Write;
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let n = (b0 << 16) | (b1 << 8) | b2;
        let _ = write!(result, "{}", CHARS[((n >> 18) & 0x3F) as usize] as char);
        let _ = write!(result, "{}", CHARS[((n >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            let _ = write!(result, "{}", CHARS[((n >> 6) & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
        if chunk.len() > 2 {
            let _ = write!(result, "{}", CHARS[(n & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_subprocess_policy_input_serialization() {
        let input = SubprocessPolicyInput {
            operation: "command_output".to_string(),
            command: "echo".to_string(),
            args: vec!["hello".to_string(), "world".to_string()],
            cwd: Some("/tmp".to_string()),
            env: None,
        };
        let json = serde_json::to_string(&input).unwrap();
        assert!(json.contains("\"operation\":\"command_output\""));
        assert!(json.contains("\"command\":\"echo\""));
        assert!(json.contains("\"args\":[\"hello\",\"world\"]"));
        assert!(json.contains("\"cwd\":\"/tmp\""));
        assert!(!json.contains("\"env\""));
    }

    #[test]
    fn test_subprocess_policy_input_with_env() {
        let mut env = HashMap::new();
        env.insert("PATH".to_string(), "/usr/bin".to_string());
        let input = SubprocessPolicyInput {
            operation: "exec".to_string(),
            command: "/bin/sh".to_string(),
            args: vec!["-c".to_string(), "ls".to_string()],
            cwd: None,
            env: Some(env),
        };
        let json = serde_json::to_string(&input).unwrap();
        assert!(json.contains("\"env\":{\"PATH\":\"/usr/bin\"}"));
        assert!(!json.contains("\"cwd\""));
    }

    #[test]
    fn test_base64_encode() {
        assert_eq!(base64_encode(b"hello"), "aGVsbG8=");
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"a"), "YQ==");
        assert_eq!(base64_encode(b"ab"), "YWI=");
        assert_eq!(base64_encode(b"abc"), "YWJj");
        assert_eq!(base64_encode(b"Hello, World!"), "SGVsbG8sIFdvcmxkIQ==");
    }

    #[test]
    fn test_subprocess_options_deserialize() {
        let json = r#"{"cwd": "/tmp", "env": {"KEY": "val"}, "encoding": "utf8"}"#;
        let opts: SubprocessOptions = serde_json::from_str(json).unwrap();
        assert_eq!(opts.cwd.as_deref(), Some("/tmp"));
        assert_eq!(opts.env.as_ref().unwrap()["KEY"], "val");
        assert_eq!(opts.encoding.as_deref(), Some("utf8"));
    }

    #[test]
    fn test_subprocess_options_defaults() {
        let json = r#"{}"#;
        let opts: SubprocessOptions = serde_json::from_str(json).unwrap();
        assert!(opts.cwd.is_none());
        assert!(opts.env.is_none());
        assert!(opts.encoding.is_none());
    }
}
