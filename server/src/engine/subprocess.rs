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
//! **Deno.Command().spawn()** (interactive / long-lived processes):
//! ```js
//! const child = new Deno.Command("codex", { args: ["app-server"] }).spawn();
//! await child.writeLine('{"method":"initialize","id":0,"params":{...}}');
//! const line = await child.readLine();   // one line of stdout, or null on EOF
//! await child.closeStdin();
//! child.kill();
//! ```
//! Unlike `output()` (which runs a command to completion and returns all of
//! stdout at once), `spawn()` keeps the child alive so JavaScript can write to
//! its stdin and read stdout incrementally — required to drive request/response
//! protocols such as the Codex `app-server` stdio (newline-delimited JSON-RPC)
//! transport, where a later request depends on an earlier response.
//!
//! Every subprocess invocation is evaluated against an OPA policy chain
//! before execution. The policy input includes the command, arguments,
//! working directory, and environment variables. `spawn()` is gated with the
//! `"command_spawn"` operation.

use std::cell::RefCell;
use std::collections::HashMap;
use std::process::Stdio;
use std::rc::Rc;
use std::sync::Arc;

use deno_core::{JsRuntime, OpState, op2};
use deno_error::JsErrorBox;
use serde::Serialize;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout};
use tokio::sync::Mutex as TokioMutex;

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

// ── Interactive (streaming) subprocesses ─────────────────────────────────
//
// `output()`/`exec()` run a command to completion. That cannot drive an
// interactive stdio protocol (e.g. Codex `app-server`), where the client must
// read a response before it can form the next request, and the server only
// flushes async responses while its stdin stays open. The registry below keeps
// a spawned child alive, keyed by a small integer handle, so separate
// spawn/write/read/close ops can operate on the same process — mirroring the
// `FsWriters` streaming-handle pattern used by `fs.createWriteStream`.

/// Registry of live spawned children, keyed by a small integer handle. Stored
/// in `OpState` so a handle survives across the separate spawn/write/read ops.
#[derive(Clone)]
pub struct Subprocesses(Arc<TokioMutex<SubprocessesInner>>);

#[derive(Default)]
struct SubprocessesInner {
    next: u32,
    map: HashMap<u32, SpawnedChild>,
}

impl Default for Subprocesses {
    fn default() -> Self {
        Self(Arc::new(TokioMutex::new(SubprocessesInner::default())))
    }
}

/// A single spawned child with piped stdio. Each stream lives behind its own
/// `Arc<TokioMutex<_>>` so a blocking read on stdout never blocks a concurrent
/// write to stdin, and the registry lock is never held across the actual I/O.
struct SpawnedChild {
    child: Arc<TokioMutex<Child>>,
    stdin: Arc<TokioMutex<Option<ChildStdin>>>,
    stdout: Arc<TokioMutex<BufReader<ChildStdout>>>,
    stderr: Arc<TokioMutex<BufReader<ChildStderr>>>,
}

fn extract_subprocesses(state: &Rc<RefCell<OpState>>) -> Result<Subprocesses, JsErrorBox> {
    let state = state.borrow();
    state
        .try_borrow::<Subprocesses>()
        .cloned()
        .ok_or_else(|| JsErrorBox::generic("subprocess: internal error — no subprocess registry available"))
}

fn join_err(e: tokio::task::JoinError) -> JsErrorBox {
    JsErrorBox::generic(format!("subprocess task join error: {}", e))
}

/// Spawn a long-lived child with piped stdin/stdout/stderr. Returns an integer
/// handle used by the write/read/close/kill ops. Gated by the `command_spawn`
/// policy operation.
#[op2(async)]
#[smi]
async fn op_subprocess_spawn(
    state: Rc<RefCell<OpState>>,
    #[string] command: String,
    #[string] args_json: String,
    #[string] options_json: String,
) -> Result<u32, JsErrorBox> {
    let policy_chain = extract_chain(&state)?;
    let subs = extract_subprocesses(&state)?;

    tokio::spawn(async move {
        let args: Vec<String> = serde_json::from_str(&args_json)
            .map_err(|e| format!("subprocess.spawn: invalid args JSON: {}", e))?;
        let options: SubprocessOptions = serde_json::from_str(&options_json)
            .map_err(|e| format!("subprocess.spawn: invalid options JSON: {}", e))?;

        check_policy(&policy_chain, "command_spawn", &command, &args, &options).await?;

        let mut cmd = tokio::process::Command::new(&command);
        cmd.args(&args);
        if let Some(ref cwd) = options.cwd {
            cmd.current_dir(cwd);
        }
        if let Some(ref env) = options.env {
            cmd.envs(env);
        }
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("subprocess.spawn: failed to execute '{}': {}", command, e))?;
        let stdin = child.stdin.take();
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "subprocess.spawn: failed to capture stdout".to_string())?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "subprocess.spawn: failed to capture stderr".to_string())?;

        let spawned = SpawnedChild {
            child: Arc::new(TokioMutex::new(child)),
            stdin: Arc::new(TokioMutex::new(stdin)),
            stdout: Arc::new(TokioMutex::new(BufReader::new(stdout))),
            stderr: Arc::new(TokioMutex::new(BufReader::new(stderr))),
        };

        let mut g = subs.0.lock().await;
        let id = g.next;
        g.next = g.next.wrapping_add(1);
        g.map.insert(id, spawned);
        Ok::<u32, String>(id)
    })
    .await
    .map_err(join_err)?
    .map_err(JsErrorBox::generic)
}

/// Write a UTF-8 string to a spawned child's stdin.
#[op2(async)]
#[string]
async fn op_subprocess_stdin_write(
    state: Rc<RefCell<OpState>>,
    #[smi] id: u32,
    #[string] data: String,
) -> Result<String, JsErrorBox> {
    let subs = extract_subprocesses(&state)?;
    let arc = {
        let g = subs.0.lock().await;
        g.map.get(&id).map(|c| c.stdin.clone())
    }
    .ok_or_else(|| JsErrorBox::generic("subprocess: invalid handle"))?;

    tokio::spawn(async move {
        let mut guard = arc.lock().await;
        match guard.as_mut() {
            Some(stdin) => {
                stdin
                    .write_all(data.as_bytes())
                    .await
                    .map_err(|e| format!("subprocess.write: {}", e))?;
                stdin
                    .flush()
                    .await
                    .map_err(|e| format!("subprocess.write: {}", e))?;
                Ok::<String, String>("{}".to_string())
            }
            None => Err("subprocess.write: stdin is already closed".to_string()),
        }
    })
    .await
    .map_err(join_err)?
    .map_err(JsErrorBox::generic)
}

/// Close a spawned child's stdin (sends EOF). Many stdio servers begin their
/// shutdown on stdin EOF, so callers should read remaining output first.
#[op2(async)]
#[string]
async fn op_subprocess_stdin_close(
    state: Rc<RefCell<OpState>>,
    #[smi] id: u32,
) -> Result<String, JsErrorBox> {
    let subs = extract_subprocesses(&state)?;
    let arc = {
        let g = subs.0.lock().await;
        g.map.get(&id).map(|c| c.stdin.clone())
    }
    .ok_or_else(|| JsErrorBox::generic("subprocess: invalid handle"))?;

    tokio::spawn(async move {
        // Dropping the ChildStdin closes the pipe → EOF for the child.
        let mut guard = arc.lock().await;
        *guard = None;
        Ok::<String, String>("{}".to_string())
    })
    .await
    .map_err(join_err)?
    .map_err(JsErrorBox::generic)
}

/// Read one line (newline-delimited) from a child's stdout. Returns a JSON
/// envelope: `{"eof":true}` at end of stream, otherwise `{"eof":false,"line":"..."}`
/// with the trailing newline stripped.
#[op2(async)]
#[string]
async fn op_subprocess_stdout_read_line(
    state: Rc<RefCell<OpState>>,
    #[smi] id: u32,
) -> Result<String, JsErrorBox> {
    let subs = extract_subprocesses(&state)?;
    let arc = {
        let g = subs.0.lock().await;
        g.map.get(&id).map(|c| c.stdout.clone())
    }
    .ok_or_else(|| JsErrorBox::generic("subprocess: invalid handle"))?;
    read_line_arc(arc).await
}

/// Read one line from a child's stderr (same envelope as stdout reads).
#[op2(async)]
#[string]
async fn op_subprocess_stderr_read_line(
    state: Rc<RefCell<OpState>>,
    #[smi] id: u32,
) -> Result<String, JsErrorBox> {
    let subs = extract_subprocesses(&state)?;
    let arc = {
        let g = subs.0.lock().await;
        g.map.get(&id).map(|c| c.stderr.clone())
    }
    .ok_or_else(|| JsErrorBox::generic("subprocess: invalid handle"))?;
    read_line_arc(arc).await
}

async fn read_line_arc<R>(arc: Arc<TokioMutex<BufReader<R>>>) -> Result<String, JsErrorBox>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut guard = arc.lock().await;
        let mut line = String::new();
        let n = guard
            .read_line(&mut line)
            .await
            .map_err(|e| format!("subprocess.readLine: {}", e))?;
        if n == 0 {
            return Ok::<String, String>("{\"eof\":true}".to_string());
        }
        while line.ends_with('\n') || line.ends_with('\r') {
            line.pop();
        }
        Ok(serde_json::json!({ "eof": false, "line": line }).to_string())
    })
    .await
    .map_err(join_err)?
    .map_err(JsErrorBox::generic)
}

/// Kill a spawned child and drop it from the registry.
#[op2(async)]
#[string]
async fn op_subprocess_kill(
    state: Rc<RefCell<OpState>>,
    #[smi] id: u32,
) -> Result<String, JsErrorBox> {
    let subs = extract_subprocesses(&state)?;
    let child = {
        let mut g = subs.0.lock().await;
        g.map.remove(&id).map(|c| c.child)
    }
    .ok_or_else(|| JsErrorBox::generic("subprocess: invalid handle"))?;

    tokio::spawn(async move {
        let mut guard = child.lock().await;
        let _ = guard.kill().await;
        Ok::<String, String>("{}".to_string())
    })
    .await
    .map_err(join_err)?
    .map_err(JsErrorBox::generic)
}

/// Wait for a spawned child to exit. Returns a JSON envelope `{"code": <int>}`
/// (exit code, or -1 if terminated by signal). Does not remove the handle;
/// callers still read any buffered output first.
#[op2(async)]
#[string]
async fn op_subprocess_wait(
    state: Rc<RefCell<OpState>>,
    #[smi] id: u32,
) -> Result<String, JsErrorBox> {
    let subs = extract_subprocesses(&state)?;
    let child = {
        let g = subs.0.lock().await;
        g.map.get(&id).map(|c| c.child.clone())
    }
    .ok_or_else(|| JsErrorBox::generic("subprocess: invalid handle"))?;

    tokio::spawn(async move {
        let mut guard = child.lock().await;
        let status = guard
            .wait()
            .await
            .map_err(|e| format!("subprocess.wait: {}", e))?;
        Ok::<String, String>(serde_json::json!({ "code": status.code().unwrap_or(-1) }).to_string())
    })
    .await
    .map_err(join_err)?
    .map_err(JsErrorBox::generic)
}

// ── Extension registration ──────────────────────────────────────────────

deno_core::extension!(
    subprocess_ext,
    ops = [
        op_subprocess_output,
        op_subprocess_exec,
        op_subprocess_spawn,
        op_subprocess_stdin_write,
        op_subprocess_stdin_close,
        op_subprocess_stdout_read_line,
        op_subprocess_stderr_read_line,
        op_subprocess_kill,
        op_subprocess_wait,
    ],
    state = |state| {
        state.put(Subprocesses::default());
    },
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

    // ── ChildProcess: interactive handle over a spawned process ─────────
    // Returned by Deno.Command#spawn(). Lets JS write to stdin and read stdout
    // incrementally, so it can drive request/response stdio protocols such as
    // the Codex app-server (newline-delimited JSON-RPC) where a later request
    // depends on an earlier response.
    function ChildProcess(idPromise) {
        this._idPromise = idPromise;
    }
    // Write a raw string to the child's stdin.
    ChildProcess.prototype.write = async function(data) {
        var id = await this._idPromise;
        await Deno.core.ops.op_subprocess_stdin_write(id, String(data));
    };
    // Write a string followed by a newline (one JSON-RPC message per line).
    ChildProcess.prototype.writeLine = async function(data) {
        return this.write(String(data) + "\n");
    };
    // Close stdin (send EOF to the child).
    ChildProcess.prototype.closeStdin = async function() {
        var id = await this._idPromise;
        await Deno.core.ops.op_subprocess_stdin_close(id);
    };
    // Read one line from stdout; resolves to the line (newline stripped) or
    // null at end of stream.
    ChildProcess.prototype.readLine = async function() {
        var id = await this._idPromise;
        var raw = await Deno.core.ops.op_subprocess_stdout_read_line(id);
        var r = JSON.parse(raw);
        return r.eof ? null : r.line;
    };
    // Read one line from stderr; null at end of stream.
    ChildProcess.prototype.readStderrLine = async function() {
        var id = await this._idPromise;
        var raw = await Deno.core.ops.op_subprocess_stderr_read_line(id);
        var r = JSON.parse(raw);
        return r.eof ? null : r.line;
    };
    // Kill the child and release the handle.
    ChildProcess.prototype.kill = async function() {
        var id = await this._idPromise;
        await Deno.core.ops.op_subprocess_kill(id);
    };
    // Wait for exit; resolves to the exit code (or -1 if signal-terminated).
    ChildProcess.prototype.status = async function() {
        var id = await this._idPromise;
        var raw = await Deno.core.ops.op_subprocess_wait(id);
        return JSON.parse(raw).code;
    };

    DenoCommand.prototype.spawn = function() {
        var args = this._options.args || [];
        if (!Array.isArray(args)) {
            throw new TypeError('Deno.Command: args must be an array');
        }
        var argsJson = JSON.stringify(args.map(String));
        var optionsJson = JSON.stringify({
            cwd: this._options.cwd || null,
            env: this._options.env || null,
        });
        // op_subprocess_spawn is async (returns a Promise for the handle id);
        // ChildProcess awaits it lazily, so spawn() stays synchronous like
        // Deno.Command#spawn().
        var idPromise = Deno.core.ops.op_subprocess_spawn(
            this._command, argsJson, optionsJson
        );
        return new ChildProcess(idPromise);
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
        // Node-ish spawn(command, args?, options?) → interactive ChildProcess
        // (see Deno.Command#spawn above for the returned handle's API).
        spawn: function(command, args, options) {
            if (typeof command !== 'string') {
                throw new TypeError('child_process.spawn: command must be a string');
            }
            var opts = options || {};
            return new DenoCommand(command, {
                args: args || [],
                cwd: opts.cwd,
                env: opts.env,
            }).spawn();
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
