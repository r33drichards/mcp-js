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
//! Hook backends mirror policy sources:
//! - `file://` URLs evaluate a Rego rule locally via `regorus`
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
    /// - `file://` → local `.rego` file or directory of `.rego` files
    pub url: String,
    /// (Remote only) REST API data path, e.g. `"mcp/fetch/pre"`. Defaults to
    /// the operation's policy path with `/pre` or `/post` appended.
    pub policy_path: Option<String>,
    /// (Local only) Regorus eval rule, e.g. `"data.mcp.fetch.pre"`. Defaults
    /// to the operation's policy rule with the trailing `.allow` replaced by
    /// `.pre` or `.post`.
    pub rule: Option<String>,
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

// ── Hook ─────────────────────────────────────────────────────────────────

/// A single hook in a chain. `Policy` wraps a whole [`PolicyChain`] as a
/// deny-only pre hook — this is how policies *are* pre hooks.
#[derive(Debug)]
pub enum Hook {
    Local(LocalHookEvaluator),
    Remote(RemoteHookEvaluator),
    Policy(Arc<PolicyChain>),
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
            Hook::Local(eval) => eval.evaluate(payload)?,
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

fn build_hook(
    source: &HookSource,
    default_remote_path: &str,
    default_local_rule: &str,
) -> Result<Hook, String> {
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
        let rule = source
            .rule
            .clone()
            .unwrap_or_else(|| default_local_rule.to_string());
        let path = Path::new(file_path);
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
        pre.push(build_hook(source, &pre_remote, &pre_rule)?);
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
        post.push(build_hook(source, &post_remote, &post_rule)?);
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
                }],
                vec![HookSource {
                    url: format!("file://{}", example.display()),
                    policy_path: None,
                    rule: None, // → data.mcp.fetch.post
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
