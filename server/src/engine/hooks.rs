//! Composable pre/post hooks around policy-gated operations.
//!
//! A [`HookChain`] generalizes the OPA policy gate: each operation runs an
//! ordered list of **pre hooks** (which see the operation input and may deny
//! it or mutate it) and **post hooks** (which see the input and output and may
//! deny the result or mutate the output). Policies are pre hooks: a configured
//! [`PolicyChain`] is appended as the *last* pre hook, so it always evaluates
//! the effective (post-mutation) input — a mutating hook can never rewrite an
//! input *after* the policy approved a different one.
//!
//! Hook backends mirror policy sources, plus a JavaScript one:
//! - `file://` URLs ending in `.js` run a JavaScript hook in a bare,
//!   capability-free V8 isolate (see [`LocalJsHookEvaluator`])
//! - other `file://` URLs evaluate a Rego rule locally via `regorus`
//! - `http://` / `https://` URLs query an OPA-style REST endpoint
//!   (`POST {base}/v1/data/{path}` with `{"input": ...}`, result unwrapped
//!   from `{"result": ...}`)
//!
//! ## Hook result contract
//!
//! A hook rule (or remote endpoint) evaluates to either:
//! - a bare boolean — `true` allows, `false` denies (pure policy behavior), or
//! - an object:
//!   - `allow` (bool, default `true`) — deny the operation when `false`
//!   - `reason` (string, optional) — human-readable denial reason
//!   - `input` (object, pre hooks only) — replacement operation input
//!   - `output` (object, post hooks only) — replacement operation output
//!
//! A Rego rule that is *undefined* for a given input abstains (allow, no
//! mutation), so partial rules compose naturally:
//!
//! ```rego
//! package mcp.fetch
//!
//! # Rewrite http:// to https:// before the policy sees it.
//! pre := {"input": object.union(input, {"url": u})} if {
//!     startswith(input.url, "http://")
//!     u := concat("", ["https://", substring(input.url, 7, -1)])
//! }
//! ```
//!
//! Pre hooks receive the (current, possibly already-mutated) operation input
//! as their Rego `input` document — the same shape policies see. Post hooks
//! receive `{"input": <operation input>, "output": <current output>}`.
//!
//! Whether a mutation is *applied* is per-operation: operations whose
//! executor cannot honor a rewritten input are built gate-only
//! ([`HookCaps::input_mutation`] = false) and fail closed if a hook attempts
//! one.

use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;

use serde::Deserialize;
use serde_json::Value;

use super::opa::{OperationPolicies, PolicyChain, build_policy_chain};

// ── Configuration ────────────────────────────────────────────────────────

/// A single hook source in `--policies-json` (`pre` / `post` arrays).
#[derive(Debug, Clone, Deserialize)]
pub struct HookSource {
    /// URL of the hook:
    /// - `http://` / `https://` → OPA-style REST endpoint
    /// - `file://` ending in `.js` → local JavaScript hook (see
    ///   [`LocalJsHookEvaluator`])
    /// - `file://` otherwise → local `.rego` file or directory of `.rego` files
    pub url: String,
    /// (Remote only) REST API data path, e.g. `"mcp/fetch/pre"`. Defaults to
    /// the operation's policy path with `/pre` or `/post` appended.
    pub policy_path: Option<String>,
    /// (Rego) eval rule, e.g. `"data.mcp.fetch.pre"` — defaults to the
    /// operation's policy rule with the trailing `.allow` replaced by `.pre`
    /// or `.post`. (JS) the global function name to call — defaults to
    /// `"pre"` / `"post"`.
    pub rule: Option<String>,
    /// (JS only) per-call timeout in milliseconds; a hook still running when
    /// it expires is terminated and the operation fails. Default 5000.
    pub timeout_ms: Option<u64>,
    /// (JS only) host capabilities granted to the hook's isolate, expressed
    /// with the same APIs the sandboxed guest sees: `"fs"` (the `fs.*`
    /// wrapper) and `"fetch"` (the `fetch()` wrapper). Default: none — the
    /// hook is pure computation. Capability-bearing hooks may be `async`.
    ///
    /// Hook-issued operations are **ungated**: they run through no hook chain
    /// and no policy, both because the hook file is operator-trusted config
    /// (the same trust as the policy files themselves) and because gating
    /// them would recurse into the very chain the hook runs inside.
    pub capabilities: Option<Vec<String>>,
}

/// What the operation's executor supports; enforced at build/run time.
#[derive(Debug, Clone, Copy)]
pub struct HookCaps {
    /// The operation applies a mutated input to its execution. When `false`,
    /// a pre hook that returns a replacement input fails the operation
    /// (fail closed) instead of being silently ignored.
    pub input_mutation: bool,
    /// The operation runs post hooks over its output. When `false`,
    /// configuring `post` hooks for the operation is a startup error.
    pub post: bool,
}

// ── Outcomes ─────────────────────────────────────────────────────────────

/// Result of running the pre-hook chain.
#[derive(Debug)]
pub enum PreOutcome {
    /// Proceed with this (possibly mutated) input.
    Allow(Value),
    /// Denied. The string is a message fragment like `"denied by policy"` or
    /// `"denied by pre hook (reason)"`, for the call site to embed in its
    /// operation-specific error.
    Deny(String),
}

/// Result of running the post-hook chain.
#[derive(Debug)]
pub enum PostOutcome {
    /// Return this (possibly mutated) output.
    Allow(Value),
    /// Denied; same message-fragment convention as [`PreOutcome::Deny`].
    Deny(String),
}

// ── Parsed hook result ───────────────────────────────────────────────────

#[derive(Debug, PartialEq)]
enum Phase {
    Pre,
    Post,
}

impl Phase {
    fn replacement_key(&self) -> &'static str {
        match self {
            Phase::Pre => "input",
            Phase::Post => "output",
        }
    }
    fn name(&self) -> &'static str {
        match self {
            Phase::Pre => "pre hook",
            Phase::Post => "post hook",
        }
    }
}

struct HookResult {
    allow: bool,
    reason: Option<String>,
    replacement: Option<Value>,
}

/// Parse the raw JSON a hook evaluated to into a [`HookResult`].
fn parse_hook_result(raw: Value, phase: &Phase) -> Result<HookResult, String> {
    match raw {
        Value::Bool(allow) => Ok(HookResult {
            allow,
            reason: None,
            replacement: None,
        }),
        Value::Object(map) => {
            let allow = match map.get("allow") {
                None => true,
                Some(Value::Bool(b)) => *b,
                Some(other) => {
                    return Err(format!(
                        "hook result field 'allow' must be a boolean, got: {}",
                        other
                    ));
                }
            };
            let reason = match map.get("reason") {
                None | Some(Value::Null) => None,
                Some(Value::String(s)) => Some(s.clone()),
                Some(other) => {
                    return Err(format!(
                        "hook result field 'reason' must be a string, got: {}",
                        other
                    ));
                }
            };
            // Reject the wrong-phase replacement key loudly: an `output` in a
            // pre hook (or `input` in a post hook) is a config mistake, not
            // something to silently drop.
            let wrong_key = match phase {
                Phase::Pre => "output",
                Phase::Post => "input",
            };
            if map.contains_key(wrong_key) {
                return Err(format!(
                    "a {} cannot set '{}' (only '{}')",
                    phase.name(),
                    wrong_key,
                    phase.replacement_key()
                ));
            }
            let replacement = match map.get(phase.replacement_key()) {
                None | Some(Value::Null) => None,
                Some(v @ Value::Object(_)) => Some(v.clone()),
                Some(other) => {
                    return Err(format!(
                        "hook result field '{}' must be an object, got: {}",
                        phase.replacement_key(),
                        other
                    ));
                }
            };
            Ok(HookResult {
                allow,
                reason,
                replacement,
            })
        }
        other => Err(format!(
            "hook must evaluate to a boolean or an object, got: {}",
            other
        )),
    }
}

// ── Local (regorus) hook evaluator ───────────────────────────────────────

/// Evaluates a Rego rule from local files, returning the rule's raw value.
/// Mirrors `opa::LocalPolicyEvaluator` but does not coerce to a boolean.
#[derive(Debug)]
pub struct LocalHookEvaluator {
    engine: Mutex<regorus::Engine>,
    eval_rule: String,
}

impl LocalHookEvaluator {
    pub fn from_file<P: AsRef<Path>>(path: P, eval_rule: String) -> Result<Self, String> {
        let path = path.as_ref();
        let source = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read rego file '{}': {}", path.display(), e))?;
        let mut engine = regorus::Engine::new();
        engine
            .add_policy(path.display().to_string(), source)
            .map_err(|e| format!("Failed to parse rego file '{}': {}", path.display(), e))?;
        Ok(Self {
            engine: Mutex::new(engine),
            eval_rule,
        })
    }

    pub fn from_directory<P: AsRef<Path>>(dir: P, eval_rule: String) -> Result<Self, String> {
        let dir = dir.as_ref();
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .map_err(|e| format!("Failed to read hook directory '{}': {}", dir.display(), e))?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("rego"))
            .collect();
        entries.sort_by_key(|e| e.file_name());

        if entries.is_empty() {
            return Err(format!("No .rego files found in '{}'", dir.display()));
        }

        let mut engine = regorus::Engine::new();
        for entry in &entries {
            let path = entry.path();
            let source = std::fs::read_to_string(&path)
                .map_err(|e| format!("Failed to read '{}': {}", path.display(), e))?;
            engine
                .add_policy(path.display().to_string(), source)
                .map_err(|e| format!("Failed to parse '{}': {}", path.display(), e))?;
        }

        Ok(Self {
            engine: Mutex::new(engine),
            eval_rule,
        })
    }

    /// Evaluate the rule against `payload`. Returns `Ok(None)` when the rule
    /// is undefined for this input (the hook abstains).
    fn evaluate(&self, payload: &Value) -> Result<Option<Value>, String> {
        // regorus::Engine is !Send; evaluation is CPU-bound and fast, so it
        // runs synchronously under the lock (same pattern as the policy path).
        let mut engine = self
            .engine
            .lock()
            .map_err(|e| format!("Hook engine lock poisoned: {}", e))?;

        engine.set_input(regorus::Value::from(payload.clone()));
        let result = engine.eval_rule(self.eval_rule.clone());
        engine.set_input(regorus::Value::new_object());

        let value = result
            .map_err(|e| format!("Rego eval failed for hook rule '{}': {}", self.eval_rule, e))?;
        if value == regorus::Value::Undefined {
            return Ok(None);
        }
        let json = serde_json::to_value(&value).map_err(|e| {
            format!(
                "Failed to convert hook rule '{}' result to JSON: {}",
                self.eval_rule, e
            )
        })?;
        Ok(Some(json))
    }
}

// ── Remote (OPA REST) hook evaluator ─────────────────────────────────────

/// Queries an OPA-style REST endpoint and returns the raw `result` document.
#[derive(Debug)]
pub struct RemoteHookEvaluator {
    client: reqwest::Client,
    /// Full URL, e.g. `http://opa:8181/v1/data/mcp/fetch/pre`.
    url: String,
}

#[derive(Deserialize)]
struct RemoteHookResponse {
    result: Option<Value>,
}

impl RemoteHookEvaluator {
    pub fn new(base_url: &str, policy_path: &str) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .expect("Failed to create hook HTTP client");
        let url = format!(
            "{}/v1/data/{}",
            base_url.trim_end_matches('/'),
            policy_path
        );
        Self { client, url }
    }

    /// Evaluate the remote hook. `Ok(None)` when the endpoint returns no
    /// `result` (the hook abstains).
    async fn evaluate(&self, payload: &Value) -> Result<Option<Value>, String> {
        let body = serde_json::json!({ "input": payload });
        let resp = self
            .client
            .post(&self.url)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Hook request to '{}' failed: {}", self.url, e))?;

        if !resp.status().is_success() {
            return Err(format!(
                "Hook endpoint '{}' returned HTTP {}",
                self.url,
                resp.status()
            ));
        }

        let parsed: RemoteHookResponse = resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse hook response from '{}': {}", self.url, e))?;
        Ok(parsed.result)
    }
}

// ── Local (JavaScript) hook evaluator ────────────────────────────────────

/// Evaluates a JavaScript hook file in a dedicated, bare V8 isolate.
///
/// The file is executed once to define its hook functions in the global
/// scope; each call then invokes one of them:
///
/// ```js
/// function pre(input) {
///     if (input.url.startsWith("http://")) {
///         return { input: { ...input, url: "https://" + input.url.slice(7) } };
///     }
///     // no return value (undefined) = abstain
/// }
///
/// function post(input, output) {
///     if (output.status >= 500) return { allow: false, reason: "upstream error" };
/// }
/// ```
///
/// Return semantics match Rego hooks: `undefined`/`null` abstains, a bool
/// allows/denies, and `{allow, reason, input|output}` denies or rewrites.
/// A hook may be `async` (or return a Promise): the worker drives the
/// isolate's event loop until it settles, still bounded by the timeout.
///
/// The isolate is created lazily on a dedicated worker thread (never on an
/// executing sandbox's isolate thread) and kept warm across calls, so
/// top-level state in the hook file persists — deliberately, for counters
/// and caches. By default it has **no host capabilities** — no `fetch`, no
/// `fs`, no ops; a JS hook is pure computation over its arguments. The
/// source's `capabilities` list opts into pieces of the guest environment,
/// with the same JS API the sandbox sees: `"fs"` installs the `fs.*`
/// wrapper, `"fetch"` installs `fetch()` (plus `atob`/`btoa`). Hook-issued
/// operations run through no hook chain and no policy: the hook file is
/// operator-trusted config, and gating them would recurse into the chain
/// the hook runs inside.
///
/// A call that exceeds the timeout fails the operation (fail closed):
/// running script is terminated via V8's thread-safe handle, and a call
/// parked on a pending host op (a hung `fetch`) is abandoned by the
/// worker's own event-loop timeout.
#[derive(Debug)]
pub struct LocalJsHookEvaluator {
    path: String,
    source: String,
    function: String,
    timeout: std::time::Duration,
    capabilities: Vec<String>,
    worker: std::sync::Mutex<Option<Result<JsWorker, String>>>,
}

#[derive(Clone)]
struct JsWorker {
    tx: std::sync::mpsc::Sender<JsCall>,
    isolate_handle: deno_core::v8::IsolateHandle,
}

impl std::fmt::Debug for JsWorker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JsWorker").finish_non_exhaustive()
    }
}

struct JsCall {
    args: Vec<Value>,
    respond: tokio::sync::oneshot::Sender<Result<Option<Value>, String>>,
}

/// Capabilities a JS hook isolate can be granted.
const JS_HOOK_CAPABILITIES: &[&str] = &["fs", "fetch"];

impl LocalJsHookEvaluator {
    pub fn from_file<P: AsRef<Path>>(
        path: P,
        function: String,
        timeout_ms: u64,
        capabilities: Vec<String>,
    ) -> Result<Self, String> {
        let path = path.as_ref();
        for cap in &capabilities {
            if !JS_HOOK_CAPABILITIES.contains(&cap.as_str()) {
                return Err(format!(
                    "JS hook '{}': unknown capability '{}' (supported: {})",
                    path.display(),
                    cap,
                    JS_HOOK_CAPABILITIES.join(", ")
                ));
            }
        }
        let source = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read JS hook file '{}': {}", path.display(), e))?;
        Ok(Self {
            path: path.display().to_string(),
            source,
            function,
            timeout: std::time::Duration::from_millis(timeout_ms),
            capabilities,
            worker: std::sync::Mutex::new(None),
        })
    }

    /// Get the warm worker, spawning it on first use. Spawning is lazy so
    /// evaluators can be constructed before V8 platform initialization.
    fn worker(&self) -> Result<JsWorker, String> {
        let mut guard = self
            .worker
            .lock()
            .map_err(|e| format!("JS hook worker lock poisoned: {}", e))?;
        if guard.is_none() {
            let (tx, rx) = std::sync::mpsc::channel::<JsCall>();
            let (setup_tx, setup_rx) =
                std::sync::mpsc::channel::<Result<deno_core::v8::IsolateHandle, String>>();
            let source = self.source.clone();
            let path = self.path.clone();
            let function = self.function.clone();
            let capabilities = self.capabilities.clone();
            let timeout = self.timeout;
            let spawned = std::thread::Builder::new()
                .name("js-hook".to_string())
                .spawn(move || {
                    js_hook_worker(source, path, function, capabilities, timeout, rx, setup_tx)
                });
            let setup = match spawned {
                Err(e) => Err(format!("Failed to spawn JS hook thread: {}", e)),
                Ok(_) => match setup_rx.recv_timeout(std::time::Duration::from_secs(10)) {
                    Ok(Ok(handle)) => Ok(JsWorker {
                        tx,
                        isolate_handle: handle,
                    }),
                    Ok(Err(e)) => Err(e),
                    Err(_) => Err(format!(
                        "JS hook '{}' failed to initialize within 10s",
                        self.path
                    )),
                },
            };
            *guard = Some(setup);
        }
        guard.as_ref().unwrap().clone()
    }

    /// Call the hook function with `args`. `Ok(None)` when the function
    /// returns `undefined` or `null` (the hook abstains).
    async fn evaluate(&self, args: Vec<Value>) -> Result<Option<Value>, String> {
        let worker = self.worker()?;
        let (respond, rx) = tokio::sync::oneshot::channel();
        worker
            .tx
            .send(JsCall { args, respond })
            .map_err(|_| format!("JS hook '{}' worker is no longer running", self.path))?;
        // The worker's own event-loop timeout fires at `self.timeout` for a
        // call parked on host ops; the grace period here lets that cleaner
        // path win. A pure-JS spin blocks the worker thread outright, so this
        // caller-side terminate is what unsticks it.
        let grace = self.timeout + std::time::Duration::from_millis(500);
        match tokio::time::timeout(grace, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(format!(
                "JS hook '{}' worker terminated unexpectedly",
                self.path
            )),
            Err(_) => {
                // Fail closed and unstick the worker for subsequent calls.
                worker.isolate_handle.terminate_execution();
                Err(format!(
                    "JS hook '{}' timed out after {:?}",
                    self.path, self.timeout
                ))
            }
        }
    }
}

/// Worker-thread body: create the isolate with the granted capabilities,
/// evaluate the hook file once, then serve calls until every sender is
/// dropped. All JS execution happens under a current-thread tokio runtime —
/// deno_core's op machinery (deno_unsync) requires one to drive async ops.
fn js_hook_worker(
    source: String,
    path: String,
    function: String,
    capabilities: Vec<String>,
    timeout: std::time::Duration,
    rx: std::sync::mpsc::Receiver<JsCall>,
    setup_tx: std::sync::mpsc::Sender<Result<deno_core::v8::IsolateHandle, String>>,
) {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            let _ = setup_tx.send(Err(format!(
                "JS hook '{}': failed to build worker runtime: {}",
                path, e
            )));
            return;
        }
    };

    let has = |c: &str| capabilities.iter().any(|x| x == c);
    let mut extensions: Vec<deno_core::Extension> = Vec::new();
    if has("fs") {
        extensions.push(super::fs::create_extension());
    }
    if has("fetch") {
        extensions.push(super::fetch::create_extension());
    }
    let mut runtime = deno_core::JsRuntime::new(deno_core::RuntimeOptions {
        extensions,
        ..Default::default()
    });
    let handle = runtime.v8_isolate().thread_safe_handle();

    // Grant capabilities with the same wrappers the guest sandbox gets, but
    // over permissive chains: hook-issued operations are ungated (see the
    // struct docs — trusted config code, and gating would recurse).
    let setup: Result<(), String> = rt.block_on(async {
        if has("fs") {
            runtime.op_state().borrow_mut().put(super::fs::FsConfig::new_with_hooks(
                Arc::new(HookChain::permissive("filesystem")),
            ));
            super::fs::inject_fs(&mut runtime)?;
        }
        if has("fetch") {
            runtime
                .op_state()
                .borrow_mut()
                .put(super::fetch::FetchConfig::new_with_hooks(Arc::new(
                    HookChain::permissive("fetch"),
                )));
            super::console::inject_base64(&mut runtime)?;
            super::fetch::inject_fetch(&mut runtime)?;
        }
        runtime
            .execute_script("<js-hook>", source)
            .map_err(|e| format!("Failed to evaluate JS hook '{}': {}", path, e))?;
        Ok(())
    });
    if let Err(e) = setup {
        let _ = setup_tx.send(Err(e));
        return;
    }
    let _ = setup_tx.send(Ok(handle));

    for call in rx {
        let result = rt.block_on(js_hook_call(
            &mut runtime,
            &function,
            &call.args,
            &path,
            timeout,
        ));
        // A timed-out call leaves the isolate in the terminated state (the
        // flag outlives the aborted script); clear it so later calls work.
        if result.is_err() {
            runtime.v8_isolate().cancel_terminate_execution();
        }
        let _ = call.respond.send(result);
    }
}

async fn js_hook_call(
    runtime: &mut deno_core::JsRuntime,
    function: &str,
    args: &[Value],
    path: &str,
    timeout: std::time::Duration,
) -> Result<Option<Value>, String> {
    use deno_core::v8;

    // Hand the arguments over as a global, then call the function by name.
    {
        deno_core::scope!(scope, runtime);
        let global = scope.get_current_context().global(scope);
        let key = v8::String::new(scope, "__hook_args")
            .ok_or_else(|| "JS hook: failed to allocate v8 string".to_string())?;
        let args_v8 = deno_core::serde_v8::to_v8(scope, args)
            .map_err(|e| format!("JS hook '{}': failed to convert arguments: {}", path, e))?;
        global.set(scope, key.into(), args_v8);
    }

    let fname = serde_json::to_string(function)
        .map_err(|e| format!("JS hook '{}': invalid function name: {}", path, e))?;
    let script = format!(
        r#"(function() {{
            const name = {fname};
            const fn = globalThis[name];
            if (typeof fn !== "function") {{
                throw new Error("JS hook function '" + name + "' is not defined");
            }}
            return fn(...globalThis.__hook_args);
        }})()"#
    );
    let result = runtime
        .execute_script("<js-hook-call>", script)
        .map_err(|e| format!("JS hook '{}' failed: {}", path, e))?;

    // An async hook returns a Promise: drive the event loop until it
    // settles, bounded by the timeout (a pure-JS spin never reaches here —
    // the caller's terminate handles that case).
    let is_promise = {
        deno_core::scope!(scope, runtime);
        v8::Local::new(scope, &result).is_promise()
    };
    let settled = if is_promise {
        tokio::time::timeout(timeout, runtime.resolve_value(result))
            .await
            .map_err(|_| format!("JS hook '{}' timed out after {:?}", path, timeout))?
            .map_err(|e| format!("JS hook '{}' failed: {}", path, e))?
    } else {
        result
    };

    deno_core::scope!(scope, runtime);
    let local = v8::Local::new(scope, settled);
    if local.is_undefined() || local.is_null() {
        return Ok(None);
    }
    deno_core::serde_v8::from_v8::<Value>(scope, local)
        .map(Some)
        .map_err(|e| format!("JS hook '{}' returned an unserializable value: {}", path, e))
}

/// Fail closed if a pre hook changed the `operation` discriminator in the
/// effective input. The executor performs the operation it was invoked for
/// regardless of the JSON field, so a rewritten `operation` could only
/// desynchronize what later hooks — and the policy, which runs last — are
/// evaluating from what will actually run.
pub fn verify_operation(effective: &Value, expected: &str, op: &str) -> Result<(), String> {
    match effective.get("operation").and_then(|v| v.as_str()) {
        Some(o) if o == expected => Ok(()),
        other => Err(format!(
            "{}: pre hook changed 'operation' (expected {:?}, got {:?})",
            op, expected, other
        )),
    }
}

// ── Hook ─────────────────────────────────────────────────────────────────

/// A single hook in a chain. `Policy` wraps a whole [`PolicyChain`] as a
/// deny-only pre hook — this is how policies *are* pre hooks.
#[derive(Debug)]
pub enum Hook {
    Local(LocalHookEvaluator),
    LocalJs(LocalJsHookEvaluator),
    Remote(RemoteHookEvaluator),
    Policy(Arc<PolicyChain>),
    /// Native fetch credential injection as a pre hook. Positioned after the
    /// configured pre hooks and before the policy, so it keys off the
    /// *effective* destination (a hook rewrite can never carry credentials
    /// to a host its rule doesn't match), the policy validates the headers
    /// that will actually be sent, and user hooks never see operator
    /// credentials at all.
    FetchHeaderInject(Vec<super::fetch::HeaderRule>),
}

impl Hook {
    /// Run this hook in `phase` over `payload`, threading `current` (the
    /// value the hook may replace). Returns the possibly-replaced value or a
    /// deny message fragment.
    async fn run(
        &self,
        phase: &Phase,
        payload: &Value,
        current: Value,
        op: &str,
        allow_mutation: bool,
    ) -> Result<Result<Value, String>, String> {
        let raw = match self {
            Hook::Policy(chain) => {
                // A policy evaluates the current effective input and never
                // mutates. Its deny keeps the historical message fragment.
                debug_assert_eq!(*phase, Phase::Pre);
                return if chain.evaluate(payload).await? {
                    Ok(Ok(current))
                } else {
                    Ok(Err("denied by policy".to_string()))
                };
            }
            Hook::FetchHeaderInject(rules) => {
                debug_assert_eq!(*phase, Phase::Pre);
                let host = payload["url_parsed"]["host"].as_str().unwrap_or("");
                let method = payload.get("method").and_then(|v| v.as_str()).unwrap_or("");
                let mut headers: std::collections::HashMap<String, String> = payload
                    .get("headers")
                    .cloned()
                    .map(serde_json::from_value)
                    .transpose()
                    .map_err(|e| format!("{}: invalid headers in hook input: {}", op, e))?
                    .unwrap_or_default();
                let injected =
                    super::fetch::apply_header_rules_tracked(rules, host, method, &mut headers)
                        .await
                        .map_err(|e| {
                            format!("{}: credential injection failed for host '{}': {}", op, host, e)
                        })?;
                if injected.is_empty() {
                    return Ok(Ok(current));
                }
                let mut replaced = current;
                replaced["headers"] = serde_json::json!(headers);
                return Ok(Ok(replaced));
            }
            Hook::Local(eval) => eval.evaluate(payload)?,
            Hook::LocalJs(eval) => {
                // JS hooks take positional arguments: pre(input) and
                // post(input, output), unwrapped from the combined payload.
                let args = match phase {
                    Phase::Pre => vec![payload.clone()],
                    Phase::Post => vec![
                        payload.get("input").cloned().unwrap_or(Value::Null),
                        payload.get("output").cloned().unwrap_or(Value::Null),
                    ],
                };
                eval.evaluate(args).await?
            }
            Hook::Remote(eval) => eval.evaluate(payload).await?,
        };

        let Some(raw) = raw else {
            // Undefined rule / absent result: the hook abstains.
            return Ok(Ok(current));
        };

        let result = parse_hook_result(raw, phase)?;
        if !result.allow {
            let deny = match result.reason {
                Some(reason) => format!("denied by {} ({})", phase.name(), reason),
                None => format!("denied by {}", phase.name()),
            };
            return Ok(Err(deny));
        }
        match result.replacement {
            None => Ok(Ok(current)),
            Some(replacement) => {
                if !allow_mutation {
                    return Err(format!(
                        "a {} attempted to mutate the {} for '{}', which does not support {} mutation",
                        phase.name(),
                        phase.replacement_key(),
                        op,
                        phase.replacement_key(),
                    ));
                }
                Ok(Ok(replacement))
            }
        }
    }
}

// ── HookChain ────────────────────────────────────────────────────────────

/// Ordered pre and post hooks for one operation.
#[derive(Debug)]
pub struct HookChain {
    /// Operation name, for error messages ("fetch", "filesystem", …).
    op: String,
    pre: Vec<Hook>,
    post: Vec<Hook>,
    /// Whether the operation's executor applies mutated inputs.
    input_mutation: bool,
}

impl HookChain {
    /// A chain with no hooks at all: allows everything, mutates nothing.
    pub fn permissive(op: impl Into<String>) -> Self {
        Self {
            op: op.into(),
            pre: Vec::new(),
            post: Vec::new(),
            input_mutation: false,
        }
    }

    /// Wrap an existing [`PolicyChain`] as the sole pre hook — the
    /// compatibility path for callers holding a bare policy chain.
    pub fn from_policy(op: impl Into<String>, chain: Arc<PolicyChain>) -> Self {
        Self {
            op: op.into(),
            pre: vec![Hook::Policy(chain)],
            post: Vec::new(),
            input_mutation: false,
        }
    }

    /// Whether any post hooks are configured (lets call sites skip building
    /// the output document when there is nothing to run).
    pub fn has_post(&self) -> bool {
        !self.post.is_empty()
    }

    /// Insert a system pre hook immediately before the trailing policy hook
    /// (or at the end when no policy is configured). This is how built-in
    /// boundary behavior — fetch credential injection — takes its place in
    /// the chain: after every configured pre hook, before the policy.
    pub fn insert_pre_before_policy(&mut self, hook: Hook) {
        let at = if matches!(self.pre.last(), Some(Hook::Policy(_))) {
            self.pre.len() - 1
        } else {
            self.pre.len()
        };
        self.pre.insert(at, hook);
    }

    /// Run the pre-hook chain over `input`.
    pub async fn run_pre(&self, input: Value) -> Result<PreOutcome, String> {
        self.run_pre_with(input, |_| Ok(())).await
    }

    /// Run the pre-hook chain, applying `normalize` to the input after every
    /// mutation. Operations with derived input fields (e.g. fetch's
    /// `url_parsed`, run_js_file's canonicalized path) use this to keep those
    /// fields consistent — and to keep the policy, which runs last, from ever
    /// seeing a stale derivation.
    pub async fn run_pre_with<F>(&self, input: Value, normalize: F) -> Result<PreOutcome, String>
    where
        F: Fn(&mut Value) -> Result<(), String>,
    {
        let mut current = input;
        for hook in &self.pre {
            let before_mutation = current.clone();
            match hook
                .run(&Phase::Pre, &before_mutation, current, &self.op, self.input_mutation)
                .await?
            {
                Ok(next) => {
                    let mutated = next != before_mutation;
                    current = next;
                    if mutated {
                        normalize(&mut current)?;
                    }
                }
                Err(deny) => return Ok(PreOutcome::Deny(deny)),
            }
        }
        Ok(PreOutcome::Allow(current))
    }

    /// Run the post-hook chain over `output`, in the context of the
    /// (effective) operation `input`. Hooks see
    /// `{"input": input, "output": <current output>}`.
    pub async fn run_post(&self, input: &Value, output: Value) -> Result<PostOutcome, String> {
        let mut current = output;
        for hook in &self.post {
            let payload = serde_json::json!({ "input": input, "output": current });
            match hook
                .run(&Phase::Post, &payload, current, &self.op, true)
                .await?
            {
                Ok(next) => current = next,
                Err(deny) => return Ok(PostOutcome::Deny(deny)),
            }
        }
        Ok(PostOutcome::Allow(current))
    }
}

// ── Builder ──────────────────────────────────────────────────────────────

/// Default per-call timeout for JS hooks.
const JS_HOOK_DEFAULT_TIMEOUT_MS: u64 = 5000;

fn build_hook(
    source: &HookSource,
    default_remote_path: &str,
    default_local_rule: &str,
    phase: &Phase,
) -> Result<Hook, String> {
    let is_js = source.url.strip_prefix("file://").is_some_and(|p| {
        Path::new(p).extension().and_then(|x| x.to_str()) == Some("js")
    });
    if !is_js && source.capabilities.is_some() {
        return Err(format!(
            "Hook '{}': 'capabilities' is only supported for JavaScript (file://*.js) hooks",
            source.url
        ));
    }
    if source.url.starts_with("http://") || source.url.starts_with("https://") {
        let policy_path = source
            .policy_path
            .as_deref()
            .unwrap_or(default_remote_path);
        Ok(Hook::Remote(RemoteHookEvaluator::new(
            &source.url,
            policy_path,
        )))
    } else if let Some(file_path) = source.url.strip_prefix("file://") {
        let path = Path::new(file_path);
        if is_js {
            // JS hook: `rule` names the global function, defaulting to the
            // phase name ("pre" / "post").
            let function = source.rule.clone().unwrap_or_else(|| {
                match phase {
                    Phase::Pre => "pre",
                    Phase::Post => "post",
                }
                .to_string()
            });
            let timeout_ms = source.timeout_ms.unwrap_or(JS_HOOK_DEFAULT_TIMEOUT_MS);
            let capabilities = source.capabilities.clone().unwrap_or_default();
            return Ok(Hook::LocalJs(LocalJsHookEvaluator::from_file(
                path, function, timeout_ms, capabilities,
            )?));
        }
        let rule = source
            .rule
            .clone()
            .unwrap_or_else(|| default_local_rule.to_string());
        if path.is_dir() {
            Ok(Hook::Local(LocalHookEvaluator::from_directory(path, rule)?))
        } else {
            Ok(Hook::Local(LocalHookEvaluator::from_file(path, rule)?))
        }
    } else {
        Err(format!(
            "Unsupported hook URL scheme: '{}'. Use http://, https://, or file://",
            source.url
        ))
    }
}

/// Derive the default remote path / local rule for a hook phase from the
/// operation's policy defaults: `mcp/fetch` → `mcp/fetch/pre`,
/// `data.mcp.fetch.allow` → `data.mcp.fetch.pre`.
fn phase_defaults(
    default_remote_path: &str,
    default_local_rule: &str,
    phase: &Phase,
) -> (String, String) {
    let suffix = match phase {
        Phase::Pre => "pre",
        Phase::Post => "post",
    };
    let remote = format!("{}/{}", default_remote_path, suffix);
    let local = match default_local_rule.strip_suffix(".allow") {
        Some(prefix) => format!("{}.{}", prefix, suffix),
        None => format!("{}.{}", default_local_rule, suffix),
    };
    (remote, local)
}

/// Build the full [`HookChain`] for one operation from its `--policies-json`
/// entry: configured pre hooks in order, then the policy chain (if any
/// policies are configured) as the final pre hook, then post hooks.
///
/// `default_remote_path` / `default_local_rule` are the operation's *policy*
/// defaults (e.g. `"mcp/fetch"` / `"data.mcp.fetch.allow"`); hook defaults
/// are derived from them per phase.
pub fn build_hook_chain(
    op: &str,
    config: &OperationPolicies,
    default_remote_path: &str,
    default_local_rule: &str,
    caps: HookCaps,
) -> Result<HookChain, String> {
    if !caps.post && !config.post.is_empty() {
        return Err(format!(
            "post hooks are not supported for operation '{}'",
            op
        ));
    }

    let (pre_remote, pre_rule) = phase_defaults(default_remote_path, default_local_rule, &Phase::Pre);
    let mut pre: Vec<Hook> = Vec::new();
    for source in &config.pre {
        pre.push(build_hook(source, &pre_remote, &pre_rule, &Phase::Pre)?);
    }

    // Policies are pre hooks — the last ones, so they gate the effective
    // input that will actually execute.
    if !config.policies.is_empty() {
        let chain = build_policy_chain(config, default_remote_path, default_local_rule)?;
        pre.push(Hook::Policy(Arc::new(chain)));
    }

    let (post_remote, post_rule) =
        phase_defaults(default_remote_path, default_local_rule, &Phase::Post);
    let mut post: Vec<Hook> = Vec::new();
    for source in &config.post {
        post.push(build_hook(source, &post_remote, &post_rule, &Phase::Post)?);
    }

    Ok(HookChain {
        op: op.to_string(),
        pre,
        post,
        input_mutation: caps.input_mutation,
    })
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::opa::{EvalMode, PolicySource};
    use std::io::Write;

    fn write_rego(dir: &std::path::Path, name: &str, content: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        path
    }

    fn op_config(
        policies: Vec<PolicySource>,
        pre: Vec<HookSource>,
        post: Vec<HookSource>,
    ) -> OperationPolicies {
        OperationPolicies {
            mode: EvalMode::All,
            policies,
            pre,
            post,
        }
    }

    fn file_hook(path: &std::path::Path, rule: &str) -> HookSource {
        HookSource {
            url: format!("file://{}", path.display()),
            policy_path: None,
            rule: Some(rule.to_string()),
            timeout_ms: None,
            capabilities: None,
        }
    }

    const CAPS_FULL: HookCaps = HookCaps {
        input_mutation: true,
        post: true,
    };
    const CAPS_GATE_ONLY: HookCaps = HookCaps {
        input_mutation: false,
        post: false,
    };

    // ── Pre hook basics ──────────────────────────────────────────────────

    #[tokio::test]
    async fn pre_hook_bool_true_allows() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_rego(
            dir.path(),
            "h.rego",
            "package t\ndefault pre = true\n",
        );
        let chain = build_hook_chain(
            "test",
            &op_config(vec![], vec![file_hook(&path, "data.t.pre")], vec![]),
            "mcp/test",
            "data.t.allow",
            CAPS_FULL,
        )
        .unwrap();

        let input = serde_json::json!({"x": 1});
        match chain.run_pre(input.clone()).await.unwrap() {
            PreOutcome::Allow(v) => assert_eq!(v, input),
            other => panic!("expected allow, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn pre_hook_bool_false_denies() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_rego(dir.path(), "h.rego", "package t\ndefault pre = false\n");
        let chain = build_hook_chain(
            "test",
            &op_config(vec![], vec![file_hook(&path, "data.t.pre")], vec![]),
            "mcp/test",
            "data.t.allow",
            CAPS_FULL,
        )
        .unwrap();

        match chain.run_pre(serde_json::json!({})).await.unwrap() {
            PreOutcome::Deny(msg) => assert_eq!(msg, "denied by pre hook"),
            other => panic!("expected deny, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn pre_hook_deny_with_reason() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_rego(
            dir.path(),
            "h.rego",
            r#"
package t
pre := {"allow": false, "reason": "quota exceeded"} if {
    input.big == true
}
"#,
        );
        let chain = build_hook_chain(
            "test",
            &op_config(vec![], vec![file_hook(&path, "data.t.pre")], vec![]),
            "mcp/test",
            "data.t.allow",
            CAPS_FULL,
        )
        .unwrap();

        match chain.run_pre(serde_json::json!({"big": true})).await.unwrap() {
            PreOutcome::Deny(msg) => assert_eq!(msg, "denied by pre hook (quota exceeded)"),
            other => panic!("expected deny, got {:?}", other),
        }
        // Rule undefined for this input → the hook abstains.
        match chain.run_pre(serde_json::json!({"big": false})).await.unwrap() {
            PreOutcome::Allow(_) => {}
            other => panic!("expected allow, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn pre_hook_mutates_input() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_rego(
            dir.path(),
            "h.rego",
            r#"
package t
pre := {"input": object.union(input, {"tagged": true})} if {
    not input.tagged
}
"#,
        );
        let chain = build_hook_chain(
            "test",
            &op_config(vec![], vec![file_hook(&path, "data.t.pre")], vec![]),
            "mcp/test",
            "data.t.allow",
            CAPS_FULL,
        )
        .unwrap();

        match chain.run_pre(serde_json::json!({"x": 1})).await.unwrap() {
            PreOutcome::Allow(v) => {
                assert_eq!(v, serde_json::json!({"x": 1, "tagged": true}))
            }
            other => panic!("expected allow, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn pre_hooks_compose_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let a = write_rego(
            dir.path(),
            "a.rego",
            r#"
package a
pre := {"input": object.union(input, {"steps": ["a"]})} if { not input.steps }
"#,
        );
        let b = write_rego(
            dir.path(),
            "b.rego",
            r#"
package b
pre := {"input": object.union(input, {"steps": array.concat(input.steps, ["b"])})} if {
    input.steps
}
"#,
        );
        let chain = build_hook_chain(
            "test",
            &op_config(
                vec![],
                vec![file_hook(&a, "data.a.pre"), file_hook(&b, "data.b.pre")],
                vec![],
            ),
            "mcp/test",
            "data.t.allow",
            CAPS_FULL,
        )
        .unwrap();

        match chain.run_pre(serde_json::json!({})).await.unwrap() {
            PreOutcome::Allow(v) => assert_eq!(v["steps"], serde_json::json!(["a", "b"])),
            other => panic!("expected allow, got {:?}", other),
        }
    }

    // ── Policies as pre hooks ────────────────────────────────────────────

    #[tokio::test]
    async fn policy_runs_after_mutating_pre_hooks() {
        // The hook rewrites method POST→GET; the policy only allows GET.
        // Because the policy is the final pre hook, the mutated input passes.
        let dir = tempfile::tempdir().unwrap();
        let hook = write_rego(
            dir.path(),
            "hook.rego",
            r#"
package h
pre := {"input": object.union(input, {"method": "GET"})} if {
    input.method == "POST"
}
"#,
        );
        let policy = write_rego(
            dir.path(),
            "policy.rego",
            r#"
package mcp.test
default allow = false
allow if { input.method == "GET" }
"#,
        );

        let chain = build_hook_chain(
            "test",
            &op_config(
                vec![PolicySource {
                    url: format!("file://{}", policy.display()),
                    policy_path: None,
                    rule: None,
                }],
                vec![file_hook(&hook, "data.h.pre")],
                vec![],
            ),
            "mcp/test",
            "data.mcp.test.allow",
            CAPS_FULL,
        )
        .unwrap();

        // POST is rewritten to GET before the policy sees it → allowed.
        match chain
            .run_pre(serde_json::json!({"method": "POST"}))
            .await
            .unwrap()
        {
            PreOutcome::Allow(v) => assert_eq!(v["method"], "GET"),
            other => panic!("expected allow, got {:?}", other),
        }

        // DELETE is untouched by the hook → the policy denies it.
        match chain
            .run_pre(serde_json::json!({"method": "DELETE"}))
            .await
            .unwrap()
        {
            PreOutcome::Deny(msg) => assert_eq!(msg, "denied by policy"),
            other => panic!("expected deny, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn from_policy_wraps_chain_as_pre_hook() {
        let dir = tempfile::tempdir().unwrap();
        let policy = write_rego(
            dir.path(),
            "p.rego",
            "package mcp.test\ndefault allow = false\nallow if { input.ok == true }\n",
        );
        let eval = crate::engine::opa::LocalPolicyEvaluator::from_file(
            &policy,
            "data.mcp.test.allow".to_string(),
        )
        .unwrap();
        let policy_chain = Arc::new(PolicyChain::new(
            vec![crate::engine::opa::PolicyEvaluatorKind::Local(eval)],
            EvalMode::All,
        ));
        let chain = HookChain::from_policy("test", policy_chain);

        match chain.run_pre(serde_json::json!({"ok": true})).await.unwrap() {
            PreOutcome::Allow(_) => {}
            other => panic!("expected allow, got {:?}", other),
        }
        match chain.run_pre(serde_json::json!({"ok": false})).await.unwrap() {
            PreOutcome::Deny(msg) => assert_eq!(msg, "denied by policy"),
            other => panic!("expected deny, got {:?}", other),
        }
    }

    // ── Mutation capability enforcement ──────────────────────────────────

    #[tokio::test]
    async fn mutation_fails_closed_on_gate_only_op() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_rego(
            dir.path(),
            "h.rego",
            r#"
package t
pre := {"input": object.union(input, {"rewritten": true})}
"#,
        );
        let chain = build_hook_chain(
            "websocket",
            &op_config(vec![], vec![file_hook(&path, "data.t.pre")], vec![]),
            "mcp/websocket",
            "data.mcp.websocket.allow",
            CAPS_GATE_ONLY,
        )
        .unwrap();

        let err = chain
            .run_pre(serde_json::json!({"url": "wss://x"}))
            .await
            .expect_err("mutation on a gate-only op must fail");
        assert!(err.contains("does not support input mutation"), "got: {err}");
    }

    #[tokio::test]
    async fn post_hooks_rejected_for_unsupported_op() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_rego(dir.path(), "h.rego", "package t\ndefault post = true\n");
        let err = build_hook_chain(
            "websocket",
            &op_config(vec![], vec![], vec![file_hook(&path, "data.t.post")]),
            "mcp/websocket",
            "data.mcp.websocket.allow",
            CAPS_GATE_ONLY,
        )
        .expect_err("post hooks on an unsupported op must fail at build");
        assert!(err.contains("post hooks are not supported"), "got: {err}");
    }

    // ── Post hooks ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn post_hook_mutates_output() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_rego(
            dir.path(),
            "h.rego",
            r#"
package t
post := {"output": object.union(input.output, {"redacted": true})} if {
    input.output.secret
}
"#,
        );
        let chain = build_hook_chain(
            "test",
            &op_config(vec![], vec![], vec![file_hook(&path, "data.t.post")]),
            "mcp/test",
            "data.t.allow",
            CAPS_FULL,
        )
        .unwrap();
        assert!(chain.has_post());

        let input = serde_json::json!({"op": "x"});
        match chain
            .run_post(&input, serde_json::json!({"secret": "hunter2"}))
            .await
            .unwrap()
        {
            PostOutcome::Allow(v) => {
                assert_eq!(v["redacted"], true);
                assert_eq!(v["secret"], "hunter2");
            }
            other => panic!("expected allow, got {:?}", other),
        }

        // No secret → rule undefined → output untouched.
        match chain
            .run_post(&input, serde_json::json!({"public": 1}))
            .await
            .unwrap()
        {
            PostOutcome::Allow(v) => assert_eq!(v, serde_json::json!({"public": 1})),
            other => panic!("expected allow, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn post_hook_sees_input_and_denies() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_rego(
            dir.path(),
            "h.rego",
            r#"
package t
post := {"allow": false, "reason": "response too large"} if {
    input.input.method == "GET"
    input.output.size > 100
}
"#,
        );
        let chain = build_hook_chain(
            "test",
            &op_config(vec![], vec![], vec![file_hook(&path, "data.t.post")]),
            "mcp/test",
            "data.t.allow",
            CAPS_FULL,
        )
        .unwrap();

        let input = serde_json::json!({"method": "GET"});
        match chain
            .run_post(&input, serde_json::json!({"size": 500}))
            .await
            .unwrap()
        {
            PostOutcome::Deny(msg) => {
                assert_eq!(msg, "denied by post hook (response too large)")
            }
            other => panic!("expected deny, got {:?}", other),
        }
        match chain
            .run_post(&input, serde_json::json!({"size": 5}))
            .await
            .unwrap()
        {
            PostOutcome::Allow(_) => {}
            other => panic!("expected allow, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn post_hooks_compose_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let a = write_rego(
            dir.path(),
            "a.rego",
            r#"
package a
post := {"output": object.union(input.output, {"n": input.output.n + 1})}
"#,
        );
        let b = write_rego(
            dir.path(),
            "b.rego",
            r#"
package b
post := {"output": object.union(input.output, {"n": input.output.n * 10})}
"#,
        );
        let chain = build_hook_chain(
            "test",
            &op_config(
                vec![],
                vec![],
                vec![file_hook(&a, "data.a.post"), file_hook(&b, "data.b.post")],
            ),
            "mcp/test",
            "data.t.allow",
            CAPS_FULL,
        )
        .unwrap();

        // (0 + 1) * 10 = 10: order matters, so composition is observable.
        match chain
            .run_post(&serde_json::json!({}), serde_json::json!({"n": 0}))
            .await
            .unwrap()
        {
            PostOutcome::Allow(v) => assert_eq!(v["n"], 10),
            other => panic!("expected allow, got {:?}", other),
        }
    }

    // ── Normalization ────────────────────────────────────────────────────

    #[tokio::test]
    async fn normalizer_runs_after_each_mutation() {
        let dir = tempfile::tempdir().unwrap();
        let hook = write_rego(
            dir.path(),
            "h.rego",
            r#"
package t
pre := {"input": object.union(input, {"url": "http://example.com/x"})} if {
    input.url != "http://example.com/x"
}
"#,
        );
        // The policy checks the derived field, which only the normalizer sets.
        let policy = write_rego(
            dir.path(),
            "p.rego",
            r#"
package mcp.test
default allow = false
allow if { input.host == "example.com" }
"#,
        );
        let chain = build_hook_chain(
            "test",
            &op_config(
                vec![PolicySource {
                    url: format!("file://{}", policy.display()),
                    policy_path: None,
                    rule: None,
                }],
                vec![file_hook(&hook, "data.t.pre")],
                vec![],
            ),
            "mcp/test",
            "data.mcp.test.allow",
            CAPS_FULL,
        )
        .unwrap();

        let outcome = chain
            .run_pre_with(
                serde_json::json!({"url": "http://old.example.org/", "host": "old.example.org"}),
                |input| {
                    let url = input["url"].as_str().unwrap_or("").to_string();
                    let parsed = url::Url::parse(&url).map_err(|e| e.to_string())?;
                    input["host"] =
                        serde_json::json!(parsed.host_str().unwrap_or("").to_string());
                    Ok(())
                },
            )
            .await
            .unwrap();
        match outcome {
            PreOutcome::Allow(v) => {
                assert_eq!(v["url"], "http://example.com/x");
                assert_eq!(v["host"], "example.com");
            }
            other => panic!("expected allow, got {:?}", other),
        }
    }

    // ── Result parsing edge cases ────────────────────────────────────────

    #[tokio::test]
    async fn pre_hook_setting_output_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_rego(
            dir.path(),
            "h.rego",
            r#"
package t
pre := {"output": {"oops": true}}
"#,
        );
        let chain = build_hook_chain(
            "test",
            &op_config(vec![], vec![file_hook(&path, "data.t.pre")], vec![]),
            "mcp/test",
            "data.t.allow",
            CAPS_FULL,
        )
        .unwrap();
        let err = chain.run_pre(serde_json::json!({})).await.unwrap_err();
        assert!(err.contains("cannot set 'output'"), "got: {err}");
    }

    #[tokio::test]
    async fn hook_returning_non_object_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_rego(dir.path(), "h.rego", "package t\npre := 42\n");
        let chain = build_hook_chain(
            "test",
            &op_config(vec![], vec![file_hook(&path, "data.t.pre")], vec![]),
            "mcp/test",
            "data.t.allow",
            CAPS_FULL,
        )
        .unwrap();
        let err = chain.run_pre(serde_json::json!({})).await.unwrap_err();
        assert!(err.contains("boolean or an object"), "got: {err}");
    }

    // ── Builder defaults ─────────────────────────────────────────────────

    #[test]
    fn phase_defaults_derive_from_policy_defaults() {
        let (remote, local) = phase_defaults("mcp/fetch", "data.mcp.fetch.allow", &Phase::Pre);
        assert_eq!(remote, "mcp/fetch/pre");
        assert_eq!(local, "data.mcp.fetch.pre");
        let (remote, local) = phase_defaults("mcp/tools", "data.mcp.tools.allow", &Phase::Post);
        assert_eq!(remote, "mcp/tools/post");
        assert_eq!(local, "data.mcp.tools.post");
    }

    #[tokio::test]
    async fn default_rule_used_when_unset() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_rego(
            dir.path(),
            "h.rego",
            "package mcp.test\ndefault pre = false\n",
        );
        let chain = build_hook_chain(
            "test",
            &op_config(
                vec![],
                vec![HookSource {
                    url: format!("file://{}", path.display()),
                    policy_path: None,
                    rule: None, // → data.mcp.test.pre
                    timeout_ms: None,
                    capabilities: None,
                }],
                vec![],
            ),
            "mcp/test",
            "data.mcp.test.allow",
            CAPS_FULL,
        )
        .unwrap();
        match chain.run_pre(serde_json::json!({})).await.unwrap() {
            PreOutcome::Deny(_) => {}
            other => panic!("expected deny, got {:?}", other),
        }
    }

    #[test]
    fn invalid_scheme_rejected() {
        let err = build_hook_chain(
            "test",
            &op_config(
                vec![],
                vec![HookSource {
                    url: "ftp://example.com/h.rego".to_string(),
                    policy_path: None,
                    rule: None,
                    timeout_ms: None,
                    capabilities: None,
                }],
                vec![],
            ),
            "mcp/test",
            "data.t.allow",
            CAPS_FULL,
        )
        .expect_err("ftp:// must be rejected");
        assert!(err.contains("Unsupported hook URL scheme"), "got: {err}");
    }

    // ── Shipped example stays valid ──────────────────────────────────────

    #[tokio::test]
    async fn shipped_fetch_hooks_example_works() {
        let example = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../policies/fetch_hooks.rego");
        let chain = build_hook_chain(
            "fetch",
            &op_config(
                vec![],
                vec![HookSource {
                    url: format!("file://{}", example.display()),
                    policy_path: None,
                    rule: None, // → data.mcp.fetch.pre
                    timeout_ms: None,
                    capabilities: None,
                }],
                vec![HookSource {
                    url: format!("file://{}", example.display()),
                    policy_path: None,
                    rule: None, // → data.mcp.fetch.post
                    timeout_ms: None,
                    capabilities: None,
                }],
            ),
            "mcp/fetch",
            "data.mcp.fetch.allow",
            CAPS_FULL,
        )
        .unwrap();

        // http:// is upgraded to https://.
        let input = serde_json::json!({
            "url": "http://example.com/x",
            "url_parsed": {"query": ""},
        });
        match chain.run_pre(input).await.unwrap() {
            PreOutcome::Allow(v) => assert_eq!(v["url"], "https://example.com/x"),
            other => panic!("expected allow, got {:?}", other),
        }

        // Credentials in the query string are refused.
        let input = serde_json::json!({
            "url": "http://example.com/x?api_key=123",
            "url_parsed": {"query": "api_key=123"},
        });
        match chain.run_pre(input).await.unwrap() {
            PreOutcome::Deny(msg) => {
                assert_eq!(msg, "denied by pre hook (credentials in query string)")
            }
            other => panic!("expected deny, got {:?}", other),
        }

        // The sensitive response header is stripped; others survive.
        let input = serde_json::json!({"url": "https://example.com/x"});
        let output = serde_json::json!({
            "status": 200,
            "headers": {"x-internal-trace": "t-1", "content-type": "text/plain"},
        });
        match chain.run_post(&input, output).await.unwrap() {
            PostOutcome::Allow(v) => {
                assert!(v["headers"].get("x-internal-trace").is_none());
                assert_eq!(v["headers"]["content-type"], "text/plain");
            }
            other => panic!("expected allow, got {:?}", other),
        }
    }

    // ── JavaScript hooks ─────────────────────────────────────────────────

    static V8_INIT: std::sync::Once = std::sync::Once::new();

    fn ensure_v8() {
        V8_INIT.call_once(|| {
            crate::engine::initialize_v8();
        });
    }

    fn write_js(dir: &std::path::Path, name: &str, content: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, content).unwrap();
        path
    }

    fn js_hook(path: &std::path::Path, timeout_ms: Option<u64>) -> HookSource {
        HookSource {
            url: format!("file://{}", path.display()),
            policy_path: None,
            rule: None,
            timeout_ms,
            capabilities: None,
        }
    }

    #[tokio::test]
    async fn js_pre_hook_mutates_denies_and_abstains() {
        ensure_v8();
        let dir = tempfile::tempdir().unwrap();
        let path = write_js(
            dir.path(),
            "hook.js",
            r#"
function pre(input) {
    if (input.method === "TRACE") {
        return { allow: false, reason: "TRACE forbidden" };
    }
    if (input.url.startsWith("http://")) {
        return { input: { ...input, url: "https://" + input.url.slice(7) } };
    }
    // undefined = abstain
}
"#,
        );
        let chain = build_hook_chain(
            "test",
            &op_config(vec![], vec![js_hook(&path, None)], vec![]),
            "mcp/test",
            "data.mcp.test.allow",
            CAPS_FULL,
        )
        .unwrap();

        match chain
            .run_pre(serde_json::json!({"url": "http://example.com/x", "method": "GET"}))
            .await
            .unwrap()
        {
            PreOutcome::Allow(v) => assert_eq!(v["url"], "https://example.com/x"),
            other => panic!("expected allow, got {:?}", other),
        }
        match chain
            .run_pre(serde_json::json!({"url": "https://example.com/x", "method": "TRACE"}))
            .await
            .unwrap()
        {
            PreOutcome::Deny(msg) => assert_eq!(msg, "denied by pre hook (TRACE forbidden)"),
            other => panic!("expected deny, got {:?}", other),
        }
        // Abstain: input passes through untouched.
        let input = serde_json::json!({"url": "https://ok.example/x", "method": "GET"});
        match chain.run_pre(input.clone()).await.unwrap() {
            PreOutcome::Allow(v) => assert_eq!(v, input),
            other => panic!("expected allow, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn js_post_hook_sees_both_args_and_mutates_output() {
        ensure_v8();
        let dir = tempfile::tempdir().unwrap();
        let path = write_js(
            dir.path(),
            "hook.js",
            r#"
function post(input, output) {
    if (input.method === "GET" && output.secret) {
        const { secret, ...rest } = output;
        return { output: { ...rest, redacted: true } };
    }
}
"#,
        );
        let chain = build_hook_chain(
            "test",
            &op_config(vec![], vec![], vec![js_hook(&path, None)]),
            "mcp/test",
            "data.mcp.test.allow",
            CAPS_FULL,
        )
        .unwrap();

        let input = serde_json::json!({"method": "GET"});
        match chain
            .run_post(&input, serde_json::json!({"secret": "hunter2", "ok": 1}))
            .await
            .unwrap()
        {
            PostOutcome::Allow(v) => {
                assert!(v.get("secret").is_none());
                assert_eq!(v["redacted"], true);
                assert_eq!(v["ok"], 1);
            }
            other => panic!("expected allow, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn js_hook_exception_fails_the_operation() {
        ensure_v8();
        let dir = tempfile::tempdir().unwrap();
        let path = write_js(
            dir.path(),
            "hook.js",
            "function pre(input) { throw new Error('boom'); }\n",
        );
        let chain = build_hook_chain(
            "test",
            &op_config(vec![], vec![js_hook(&path, None)], vec![]),
            "mcp/test",
            "data.mcp.test.allow",
            CAPS_FULL,
        )
        .unwrap();
        let err = chain.run_pre(serde_json::json!({})).await.unwrap_err();
        assert!(err.contains("boom"), "got: {err}");
    }

    #[tokio::test]
    async fn js_hook_missing_function_errors() {
        ensure_v8();
        let dir = tempfile::tempdir().unwrap();
        let path = write_js(dir.path(), "hook.js", "function unrelated() {}\n");
        let chain = build_hook_chain(
            "test",
            &op_config(vec![], vec![js_hook(&path, None)], vec![]),
            "mcp/test",
            "data.mcp.test.allow",
            CAPS_FULL,
        )
        .unwrap();
        let err = chain.run_pre(serde_json::json!({})).await.unwrap_err();
        assert!(err.contains("'pre' is not defined"), "got: {err}");
    }

    #[tokio::test]
    async fn js_hook_state_persists_across_calls() {
        ensure_v8();
        let dir = tempfile::tempdir().unwrap();
        let path = write_js(
            dir.path(),
            "hook.js",
            r#"
let calls = 0;
function pre(input) {
    calls += 1;
    return { input: { ...input, call_number: calls } };
}
"#,
        );
        let chain = build_hook_chain(
            "test",
            &op_config(vec![], vec![js_hook(&path, None)], vec![]),
            "mcp/test",
            "data.mcp.test.allow",
            CAPS_FULL,
        )
        .unwrap();

        for expected in 1..=3 {
            match chain.run_pre(serde_json::json!({})).await.unwrap() {
                PreOutcome::Allow(v) => assert_eq!(v["call_number"], expected),
                other => panic!("expected allow, got {:?}", other),
            }
        }
    }

    #[tokio::test]
    async fn js_hook_timeout_terminates_and_worker_recovers() {
        ensure_v8();
        let dir = tempfile::tempdir().unwrap();
        let path = write_js(
            dir.path(),
            "hook.js",
            r#"
function pre(input) {
    if (input.spin) { for (;;) {} }
    return true;
}
"#,
        );
        let chain = build_hook_chain(
            "test",
            &op_config(vec![], vec![js_hook(&path, Some(250))], vec![]),
            "mcp/test",
            "data.mcp.test.allow",
            CAPS_FULL,
        )
        .unwrap();

        let err = chain
            .run_pre(serde_json::json!({"spin": true}))
            .await
            .unwrap_err();
        assert!(err.contains("timed out"), "got: {err}");

        // The isolate was terminated and resumed; the next call works.
        match chain.run_pre(serde_json::json!({"spin": false})).await.unwrap() {
            PreOutcome::Allow(_) => {}
            other => panic!("expected allow after recovery, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn js_hook_may_be_async() {
        ensure_v8();
        let dir = tempfile::tempdir().unwrap();
        let path = write_js(
            dir.path(),
            "hook.js",
            r#"
async function pre(input) {
    if (input.block) return { allow: false, reason: "async deny" };
    return { input: { ...input, tagged: true } };
}
"#,
        );
        let chain = build_hook_chain(
            "test",
            &op_config(vec![], vec![js_hook(&path, None)], vec![]),
            "mcp/test",
            "data.mcp.test.allow",
            CAPS_FULL,
        )
        .unwrap();
        match chain.run_pre(serde_json::json!({"block": false})).await.unwrap() {
            PreOutcome::Allow(v) => assert_eq!(v["tagged"], true),
            other => panic!("expected allow, got {:?}", other),
        }
        match chain.run_pre(serde_json::json!({"block": true})).await.unwrap() {
            PreOutcome::Deny(msg) => assert_eq!(msg, "denied by pre hook (async deny)"),
            other => panic!("expected deny, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn js_hook_with_fs_capability_writes_audit_log() {
        ensure_v8();
        let dir = tempfile::tempdir().unwrap();
        let log_path = dir.path().join("audit.log");
        let path = write_js(
            dir.path(),
            "hook.js",
            &format!(
                r#"
const LOG = {log:?};
async function pre(input) {{
    await fs.appendFile(LOG, input.operation + " " + input.path + "\n");
    // abstain: observe, don't gate
}}
"#,
                log = log_path.to_string_lossy(),
            ),
        );
        let mut source = js_hook(&path, None);
        source.capabilities = Some(vec!["fs".to_string()]);
        let chain = build_hook_chain(
            "filesystem",
            &op_config(vec![], vec![source], vec![]),
            "mcp/filesystem",
            "data.mcp.filesystem.allow",
            CAPS_FULL,
        )
        .unwrap();

        for (op, p) in [("writeFile", "/data/a.txt"), ("readFile", "/data/b.txt")] {
            match chain
                .run_pre(serde_json::json!({"operation": op, "path": p}))
                .await
                .unwrap()
            {
                PreOutcome::Allow(_) => {}
                other => panic!("expected allow, got {:?}", other),
            }
        }
        let log = std::fs::read_to_string(&log_path).unwrap();
        assert_eq!(log, "writeFile /data/a.txt\nreadFile /data/b.txt\n");
    }

    #[tokio::test]
    async fn js_hook_unknown_capability_is_a_startup_error() {
        ensure_v8();
        let dir = tempfile::tempdir().unwrap();
        let path = write_js(dir.path(), "hook.js", "function pre(input) {}\n");
        let mut source = js_hook(&path, None);
        source.capabilities = Some(vec!["subprocess".to_string()]);
        let err = build_hook_chain(
            "test",
            &op_config(vec![], vec![source], vec![]),
            "mcp/test",
            "data.mcp.test.allow",
            CAPS_FULL,
        )
        .unwrap_err();
        assert!(err.contains("unknown capability 'subprocess'"), "got: {err}");
    }

    #[tokio::test]
    async fn capabilities_on_rego_hook_is_a_startup_error() {
        ensure_v8();
        let dir = tempfile::tempdir().unwrap();
        let rego = dir.path().join("hook.rego");
        std::fs::write(&rego, "package mcp.test\n").unwrap();
        let mut source = js_hook(&rego, None);
        source.capabilities = Some(vec!["fs".to_string()]);
        let err = build_hook_chain(
            "test",
            &op_config(vec![], vec![source], vec![]),
            "mcp/test",
            "data.mcp.test.allow",
            CAPS_FULL,
        )
        .unwrap_err();
        assert!(
            err.contains("only supported for JavaScript"),
            "got: {err}"
        );
    }

    #[tokio::test]
    async fn shipped_audit_fs_hooks_js_example_evaluates() {
        ensure_v8();
        let example = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../policies/audit_fs_hooks.js");
        let mut source = js_hook(&example, None);
        source.capabilities = Some(vec!["fs".to_string()]);
        let chain = build_hook_chain(
            "filesystem",
            &op_config(vec![], vec![source], vec![]),
            "mcp/filesystem",
            "data.mcp.filesystem.allow",
            CAPS_FULL,
        )
        .unwrap();
        // A read op is not audited (no fs access), so this exercises the
        // example's eval + call path without touching its log location.
        match chain
            .run_pre(serde_json::json!({"operation": "readFile", "path": "/x"}))
            .await
            .unwrap()
        {
            PreOutcome::Allow(_) => {}
            other => panic!("expected allow, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn shipped_fetch_hooks_js_example_works() {
        ensure_v8();
        let example = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../policies/fetch_hooks.js");
        let chain = build_hook_chain(
            "fetch",
            &op_config(
                vec![],
                vec![js_hook(&example, None)],
                vec![js_hook(&example, None)],
            ),
            "mcp/fetch",
            "data.mcp.fetch.allow",
            CAPS_FULL,
        )
        .unwrap();

        // http:// is upgraded to https://.
        let input = serde_json::json!({
            "url": "http://example.com/x",
            "url_parsed": {"query": ""},
        });
        match chain.run_pre(input).await.unwrap() {
            PreOutcome::Allow(v) => assert_eq!(v["url"], "https://example.com/x"),
            other => panic!("expected allow, got {:?}", other),
        }

        // Credentials in the query string are refused.
        let input = serde_json::json!({
            "url": "https://example.com/x?api_key=123",
            "url_parsed": {"query": "api_key=123"},
        });
        match chain.run_pre(input).await.unwrap() {
            PreOutcome::Deny(msg) => {
                assert_eq!(msg, "denied by pre hook (credentials in query string)")
            }
            other => panic!("expected deny, got {:?}", other),
        }

        // The sensitive response header is stripped; others survive.
        let input = serde_json::json!({"url": "https://example.com/x"});
        let output = serde_json::json!({
            "status": 200,
            "headers": {"x-internal-trace": "t-1", "content-type": "text/plain"},
        });
        match chain.run_post(&input, output).await.unwrap() {
            PostOutcome::Allow(v) => {
                assert!(v["headers"].get("x-internal-trace").is_none());
                assert_eq!(v["headers"]["content-type"], "text/plain");
            }
            other => panic!("expected allow, got {:?}", other),
        }
    }

    // ── verify_operation ─────────────────────────────────────────────────

    #[test]
    fn verify_operation_accepts_match_and_rejects_spoof() {
        let ok = serde_json::json!({"operation": "writeFile", "path": "/x"});
        assert!(verify_operation(&ok, "writeFile", "fs.writeFile").is_ok());

        let spoofed = serde_json::json!({"operation": "readFile", "path": "/x"});
        let err = verify_operation(&spoofed, "writeFile", "fs.writeFile").unwrap_err();
        assert!(err.contains("pre hook changed 'operation'"), "got: {err}");

        let dropped = serde_json::json!({"path": "/x"});
        let err = verify_operation(&dropped, "writeFile", "fs.writeFile").unwrap_err();
        assert!(err.contains("got None"), "got: {err}");
    }

    // ── Remote hooks (OPA-style HTTP endpoint) ───────────────────────────

    #[tokio::test]
    async fn remote_hook_mutates_and_denies() {
        use axum::{Json, Router, routing::post};

        // A tiny OPA-shaped endpoint: rewrites method HEAD→GET, denies TRACE.
        async fn pre_handler(Json(body): Json<Value>) -> Json<Value> {
            let input = &body["input"];
            let result = match input["method"].as_str() {
                Some("TRACE") => {
                    serde_json::json!({"allow": false, "reason": "TRACE forbidden"})
                }
                Some("HEAD") => {
                    let mut mutated = input.clone();
                    mutated["method"] = serde_json::json!("GET");
                    serde_json::json!({"input": mutated})
                }
                _ => serde_json::json!(true),
            };
            Json(serde_json::json!({"result": result}))
        }

        let app = Router::new().route("/v1/data/mcp/test/pre", post(pre_handler));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let chain = build_hook_chain(
            "test",
            &op_config(
                vec![],
                vec![HookSource {
                    url: format!("http://{}", addr),
                    policy_path: None, // → mcp/test/pre
                    rule: None,
                    timeout_ms: None,
                    capabilities: None,
                }],
                vec![],
            ),
            "mcp/test",
            "data.mcp.test.allow",
            CAPS_FULL,
        )
        .unwrap();

        match chain
            .run_pre(serde_json::json!({"method": "HEAD"}))
            .await
            .unwrap()
        {
            PreOutcome::Allow(v) => assert_eq!(v["method"], "GET"),
            other => panic!("expected allow, got {:?}", other),
        }
        match chain
            .run_pre(serde_json::json!({"method": "TRACE"}))
            .await
            .unwrap()
        {
            PreOutcome::Deny(msg) => assert_eq!(msg, "denied by pre hook (TRACE forbidden)"),
            other => panic!("expected deny, got {:?}", other),
        }
        match chain
            .run_pre(serde_json::json!({"method": "GET"}))
            .await
            .unwrap()
        {
            PreOutcome::Allow(v) => assert_eq!(v["method"], "GET"),
            other => panic!("expected allow, got {:?}", other),
        }
    }
}
