//! Open Policy Agent (OPA) policy evaluation.
//!
//! Supports two evaluator backends:
//! - **Remote**: queries an OPA REST API server (`http://` / `https://` URLs)
//! - **Local**: evaluates Rego files on disk via `regorus` (`file://` URLs)
//!
//! Multiple evaluators can be composed into a [`PolicyChain`] with configurable
//! evaluation mode (all-must-allow or any-allows).

use std::path::Path;
use std::sync::Mutex;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

// ── PolicyEvaluator trait ────────────────────────────────────────────────

/// A single policy evaluator. Implementations include remote OPA servers
/// and local Rego file evaluation.
#[async_trait]
pub trait PolicyEvaluator: Send + Sync + std::fmt::Debug {
    /// Evaluate the policy with the given input. Returns `Ok(true)` if allowed.
    async fn evaluate_policy(&self, input: &serde_json::Value) -> Result<bool, String>;
}

// ── Remote (OPA REST API) evaluator ──────────────────────────────────────

/// Async OPA client that queries a remote OPA server via its REST API.
#[derive(Clone, Debug)]
pub struct OpaClient {
    base_url: String,
    client: reqwest::Client,
}

#[derive(Serialize)]
struct OpaRequest<T: Serialize> {
    input: T,
}

#[derive(Deserialize)]
struct OpaResponse {
    result: Option<OpaResultBody>,
}

#[derive(Deserialize)]
struct OpaResultBody {
    allow: Option<bool>,
}

impl OpaClient {
    pub fn new(base_url: String) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .expect("Failed to create OPA HTTP client");
        Self { base_url, client }
    }

    /// Evaluate an OPA policy. Returns `Ok(true)` if the policy allows the
    /// operation, `Ok(false)` if denied, or `Err` on connectivity / parse errors.
    ///
    /// `policy_path` is appended to `/v1/data/` — e.g. `"mcp/fetch"` becomes
    /// `POST {base_url}/v1/data/mcp/fetch`.
    pub async fn evaluate<T: Serialize>(&self, policy_path: &str, input: &T) -> Result<bool, String> {
        let url = format!("{}/v1/data/{}", self.base_url.trim_end_matches('/'), policy_path);
        let body = OpaRequest { input };

        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("OPA request failed: {}", e))?;

        if !resp.status().is_success() {
            return Err(format!("OPA returned HTTP {}", resp.status()));
        }

        let opa_resp: OpaResponse = resp
            .json()
            .await
            .map_err(|e| format!("Failed to parse OPA response: {}", e))?;

        Ok(opa_resp
            .result
            .and_then(|r| r.allow)
            .unwrap_or(false))
    }
}

/// Wraps an [`OpaClient`] + policy path as a [`PolicyEvaluator`].
#[derive(Debug)]
pub struct RemotePolicyEvaluator {
    client: OpaClient,
    policy_path: String,
}

impl RemotePolicyEvaluator {
    pub fn new(base_url: String, policy_path: String) -> Self {
        Self {
            client: OpaClient::new(base_url),
            policy_path,
        }
    }
}

#[async_trait]
impl PolicyEvaluator for RemotePolicyEvaluator {
    async fn evaluate_policy(&self, input: &serde_json::Value) -> Result<bool, String> {
        self.client.evaluate(&self.policy_path, input).await
    }
}

// ── Local (regorus) evaluator ────────────────────────────────────────────

/// Evaluates Rego policies from local files using the `regorus` engine.
/// The engine is wrapped in a `Mutex` because `regorus::Engine` is `!Sync`.
#[derive(Debug)]
pub struct LocalPolicyEvaluator {
    engine: Mutex<regorus::Engine>,
    eval_rule: String,
}

impl LocalPolicyEvaluator {
    /// Create from a single `.rego` file.
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

    /// Create from a directory of `.rego` files (non-recursive, sorted alphabetically).
    pub fn from_directory<P: AsRef<Path>>(dir: P, eval_rule: String) -> Result<Self, String> {
        let dir = dir.as_ref();
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .map_err(|e| format!("Failed to read policy directory '{}': {}", dir.display(), e))?
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path().extension().and_then(|x| x.to_str()) == Some("rego")
            })
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
}

#[async_trait]
impl PolicyEvaluator for LocalPolicyEvaluator {
    async fn evaluate_policy(&self, input: &serde_json::Value) -> Result<bool, String> {
        let input_clone = input.clone();
        let eval_rule = self.eval_rule.clone();

        // regorus::Engine is !Send, so we must evaluate under the lock
        // synchronously. The regorus evaluation is CPU-bound and fast.
        let mut engine = self.engine.lock().map_err(|e| format!("Policy engine lock poisoned: {}", e))?;

        let regorus_input = regorus::Value::from(input_clone);
        engine.set_input(regorus_input);

        let result = engine
            .eval_rule(eval_rule.clone())
            .map_err(|e| format!("Rego eval failed for rule '{}': {}", eval_rule, e))?;

        engine.set_input(regorus::Value::new_object());

        Ok(result == regorus::Value::from(true))
    }
}

// ── PolicyChain ──────────────────────────────────────────────────────────

/// How multiple policy evaluators are composed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EvalMode {
    /// All evaluators must return `true` (AND logic). Default.
    All,
    /// Any evaluator returning `true` is sufficient (OR logic).
    Any,
}

impl Default for EvalMode {
    fn default() -> Self {
        Self::All
    }
}

/// An ordered list of [`PolicyEvaluator`]s with a configurable [`EvalMode`].
#[derive(Debug)]
pub struct PolicyChain {
    evaluators: Vec<Box<dyn PolicyEvaluator>>,
    mode: EvalMode,
}

impl PolicyChain {
    pub fn new(evaluators: Vec<Box<dyn PolicyEvaluator>>, mode: EvalMode) -> Self {
        Self { evaluators, mode }
    }

    /// Evaluate the policy chain. Returns `Ok(true)` if the chain allows.
    /// An empty chain is permissive (returns `true`).
    pub async fn evaluate(&self, input: &serde_json::Value) -> Result<bool, String> {
        if self.evaluators.is_empty() {
            return Ok(true);
        }

        match self.mode {
            EvalMode::All => {
                for evaluator in &self.evaluators {
                    if !evaluator.evaluate_policy(input).await? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            EvalMode::Any => {
                for evaluator in &self.evaluators {
                    if evaluator.evaluate_policy(input).await? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
        }
    }
}

// ── JSON configuration types ─────────────────────────────────────────────

/// Top-level `--policies-json` configuration. Keys are operation names.
#[derive(Debug, Clone, Deserialize)]
pub struct PoliciesConfig {
    /// Policy chain for `fetch()` operations.
    pub fetch: Option<OperationPolicies>,
    /// Policy chain for module import auditing.
    pub modules: Option<OperationPolicies>,
}

/// Per-operation policy configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct OperationPolicies {
    /// Evaluation mode: `"all"` (default) or `"any"`.
    #[serde(default)]
    pub mode: EvalMode,
    /// Ordered list of policy sources.
    pub policies: Vec<PolicySource>,
}

/// A single policy source — either a remote OPA server or a local Rego file/directory.
#[derive(Debug, Clone, Deserialize)]
pub struct PolicySource {
    /// URL of the policy source:
    /// - `http://` / `https://` → remote OPA REST API
    /// - `file://` → local `.rego` file or directory of `.rego` files
    pub url: String,
    /// (Remote only) OPA REST API data path, e.g. `"mcp/fetch"`.
    /// Defaults are per-operation: `"mcp/fetch"` for fetch, `"mcp/modules"` for modules.
    pub policy_path: Option<String>,
    /// (Local only) Regorus eval rule, e.g. `"data.mcp.fetch.allow"`.
    /// Defaults are per-operation.
    pub rule: Option<String>,
}

/// Build a [`PolicyChain`] from a list of [`PolicySource`]s.
///
/// `default_remote_path` is the OPA REST API path used when `policy_path` is omitted.
/// `default_local_rule` is the regorus eval rule used when `rule` is omitted.
pub fn build_policy_chain(
    op_policies: &OperationPolicies,
    default_remote_path: &str,
    default_local_rule: &str,
) -> Result<PolicyChain, String> {
    let mut evaluators: Vec<Box<dyn PolicyEvaluator>> = Vec::new();

    for source in &op_policies.policies {
        if source.url.starts_with("http://") || source.url.starts_with("https://") {
            let policy_path = source
                .policy_path
                .clone()
                .unwrap_or_else(|| default_remote_path.to_string());
            evaluators.push(Box::new(RemotePolicyEvaluator::new(
                source.url.clone(),
                policy_path,
            )));
        } else if let Some(file_path) = source.url.strip_prefix("file://") {
            let rule = source
                .rule
                .clone()
                .unwrap_or_else(|| default_local_rule.to_string());

            let path = Path::new(file_path);
            if path.is_dir() {
                evaluators.push(Box::new(
                    LocalPolicyEvaluator::from_directory(path, rule)?,
                ));
            } else {
                evaluators.push(Box::new(
                    LocalPolicyEvaluator::from_file(path, rule)?,
                ));
            }
        } else {
            return Err(format!(
                "Unsupported policy URL scheme: '{}'. Use http://, https://, or file://",
                source.url
            ));
        }
    }

    Ok(PolicyChain::new(evaluators, op_policies.mode))
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn allow_rego() -> &'static str {
        r#"
package mcp.test
default allow = true
"#
    }

    fn deny_rego() -> &'static str {
        r#"
package mcp.test
default allow = false
"#
    }

    fn conditional_rego() -> &'static str {
        r#"
package mcp.fetch

default allow = false

allow if {
    input.method == "GET"
}
"#
    }

    fn write_rego_file(dir: &std::path::Path, name: &str, content: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        path
    }

    // ── LocalPolicyEvaluator tests ───────────────────────────────────────

    #[tokio::test]
    async fn test_local_evaluator_allow() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_rego_file(dir.path(), "allow.rego", allow_rego());

        let eval = LocalPolicyEvaluator::from_file(&path, "data.mcp.test.allow".to_string()).unwrap();
        let input = serde_json::json!({"method": "GET"});
        assert!(eval.evaluate_policy(&input).await.unwrap());
    }

    #[tokio::test]
    async fn test_local_evaluator_deny() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_rego_file(dir.path(), "deny.rego", deny_rego());

        let eval = LocalPolicyEvaluator::from_file(&path, "data.mcp.test.allow".to_string()).unwrap();
        let input = serde_json::json!({"method": "GET"});
        assert!(!eval.evaluate_policy(&input).await.unwrap());
    }

    #[tokio::test]
    async fn test_local_evaluator_conditional() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_rego_file(dir.path(), "cond.rego", conditional_rego());

        let eval = LocalPolicyEvaluator::from_file(&path, "data.mcp.fetch.allow".to_string()).unwrap();

        let get_input = serde_json::json!({"method": "GET"});
        assert!(eval.evaluate_policy(&get_input).await.unwrap());

        let post_input = serde_json::json!({"method": "POST"});
        assert!(!eval.evaluate_policy(&post_input).await.unwrap());
    }

    #[tokio::test]
    async fn test_local_evaluator_from_directory() {
        let dir = tempfile::tempdir().unwrap();
        // Two policies in the same package — rules combine.
        write_rego_file(dir.path(), "a_base.rego", r#"
package mcp.test
default allow = false
allow if { input.ok == true }
"#);
        write_rego_file(dir.path(), "b_extra.rego", r#"
package mcp.test
allow if { input.admin == true }
"#);
        // Non-rego file should be ignored.
        write_rego_file(dir.path(), "notes.txt", "not a policy");

        let eval = LocalPolicyEvaluator::from_directory(dir.path(), "data.mcp.test.allow".to_string()).unwrap();

        let ok_input = serde_json::json!({"ok": true});
        assert!(eval.evaluate_policy(&ok_input).await.unwrap());

        let admin_input = serde_json::json!({"admin": true});
        assert!(eval.evaluate_policy(&admin_input).await.unwrap());

        let denied_input = serde_json::json!({"ok": false});
        assert!(!eval.evaluate_policy(&denied_input).await.unwrap());
    }

    #[tokio::test]
    async fn test_local_evaluator_empty_dir_errors() {
        let dir = tempfile::tempdir().unwrap();
        let result = LocalPolicyEvaluator::from_directory(dir.path(), "data.mcp.test.allow".to_string());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("No .rego files"));
    }

    // ── PolicyChain tests ────────────────────────────────────────────────

    /// Helper evaluator that always returns a fixed value.
    #[derive(Debug)]
    struct FixedEvaluator(bool);

    #[async_trait]
    impl PolicyEvaluator for FixedEvaluator {
        async fn evaluate_policy(&self, _input: &serde_json::Value) -> Result<bool, String> {
            Ok(self.0)
        }
    }

    #[tokio::test]
    async fn test_chain_all_mode_all_allow() {
        let chain = PolicyChain::new(
            vec![Box::new(FixedEvaluator(true)), Box::new(FixedEvaluator(true))],
            EvalMode::All,
        );
        assert!(chain.evaluate(&serde_json::json!({})).await.unwrap());
    }

    #[tokio::test]
    async fn test_chain_all_mode_one_denies() {
        let chain = PolicyChain::new(
            vec![Box::new(FixedEvaluator(true)), Box::new(FixedEvaluator(false))],
            EvalMode::All,
        );
        assert!(!chain.evaluate(&serde_json::json!({})).await.unwrap());
    }

    #[tokio::test]
    async fn test_chain_any_mode_one_allows() {
        let chain = PolicyChain::new(
            vec![Box::new(FixedEvaluator(false)), Box::new(FixedEvaluator(true))],
            EvalMode::Any,
        );
        assert!(chain.evaluate(&serde_json::json!({})).await.unwrap());
    }

    #[tokio::test]
    async fn test_chain_any_mode_all_deny() {
        let chain = PolicyChain::new(
            vec![Box::new(FixedEvaluator(false)), Box::new(FixedEvaluator(false))],
            EvalMode::Any,
        );
        assert!(!chain.evaluate(&serde_json::json!({})).await.unwrap());
    }

    #[tokio::test]
    async fn test_chain_empty_allows() {
        let chain = PolicyChain::new(vec![], EvalMode::All);
        assert!(chain.evaluate(&serde_json::json!({})).await.unwrap());

        let chain = PolicyChain::new(vec![], EvalMode::Any);
        assert!(chain.evaluate(&serde_json::json!({})).await.unwrap());
    }

    // ── build_policy_chain tests ─────────────────────────────────────────

    #[test]
    fn test_build_chain_file_url() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_rego_file(dir.path(), "test.rego", allow_rego());

        let op = OperationPolicies {
            mode: EvalMode::All,
            policies: vec![PolicySource {
                url: format!("file://{}", path.display()),
                policy_path: None,
                rule: None,
            }],
        };
        let chain = build_policy_chain(&op, "mcp/fetch", "data.mcp.test.allow").unwrap();
        assert_eq!(chain.evaluators.len(), 1);
    }

    #[test]
    fn test_build_chain_directory_url() {
        let dir = tempfile::tempdir().unwrap();
        write_rego_file(dir.path(), "policy.rego", allow_rego());

        let op = OperationPolicies {
            mode: EvalMode::All,
            policies: vec![PolicySource {
                url: format!("file://{}", dir.path().display()),
                policy_path: None,
                rule: None,
            }],
        };
        let chain = build_policy_chain(&op, "mcp/fetch", "data.mcp.test.allow").unwrap();
        assert_eq!(chain.evaluators.len(), 1);
    }

    #[test]
    fn test_build_chain_remote_url() {
        let op = OperationPolicies {
            mode: EvalMode::Any,
            policies: vec![PolicySource {
                url: "http://localhost:8181".to_string(),
                policy_path: Some("custom/path".to_string()),
                rule: None,
            }],
        };
        let chain = build_policy_chain(&op, "mcp/fetch", "data.mcp.fetch.allow").unwrap();
        assert_eq!(chain.evaluators.len(), 1);
        assert_eq!(chain.mode, EvalMode::Any);
    }

    #[test]
    fn test_build_chain_invalid_scheme() {
        let op = OperationPolicies {
            mode: EvalMode::All,
            policies: vec![PolicySource {
                url: "ftp://example.com/policy.rego".to_string(),
                policy_path: None,
                rule: None,
            }],
        };
        let result = build_policy_chain(&op, "mcp/fetch", "data.mcp.fetch.allow");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unsupported policy URL scheme"));
    }

    // ── PoliciesConfig deserialization ────────────────────────────────────

    #[test]
    fn test_policies_config_deserialize() {
        let json = r#"{
            "fetch": {
                "mode": "all",
                "policies": [
                    {"url": "http://opa:8181", "policy_path": "mcp/fetch"},
                    {"url": "file:///etc/policies/fetch.rego"}
                ]
            },
            "modules": {
                "mode": "any",
                "policies": [
                    {"url": "file:///etc/policies/", "rule": "data.mcp.modules.allow"}
                ]
            }
        }"#;
        let config: PoliciesConfig = serde_json::from_str(json).unwrap();

        let fetch = config.fetch.unwrap();
        assert_eq!(fetch.mode, EvalMode::All);
        assert_eq!(fetch.policies.len(), 2);
        assert_eq!(fetch.policies[0].policy_path.as_deref(), Some("mcp/fetch"));

        let modules = config.modules.unwrap();
        assert_eq!(modules.mode, EvalMode::Any);
        assert_eq!(modules.policies.len(), 1);
        assert_eq!(modules.policies[0].rule.as_deref(), Some("data.mcp.modules.allow"));
    }

    #[test]
    fn test_policies_config_defaults() {
        let json = r#"{
            "fetch": {
                "policies": [{"url": "file:///tmp/test.rego"}]
            }
        }"#;
        let config: PoliciesConfig = serde_json::from_str(json).unwrap();
        let fetch = config.fetch.unwrap();
        assert_eq!(fetch.mode, EvalMode::All); // default
        assert!(config.modules.is_none());
    }

    // ── Integration: local rego through build_policy_chain + evaluate ────

    #[tokio::test]
    async fn test_local_chain_end_to_end() {
        let dir = tempfile::tempdir().unwrap();
        write_rego_file(dir.path(), "fetch.rego", conditional_rego());

        let op = OperationPolicies {
            mode: EvalMode::All,
            policies: vec![PolicySource {
                url: format!("file://{}", dir.path().join("fetch.rego").display()),
                policy_path: None,
                rule: None, // will use default
            }],
        };
        let chain = build_policy_chain(&op, "mcp/fetch", "data.mcp.fetch.allow").unwrap();

        let get_input = serde_json::json!({"method": "GET", "url": "https://example.com"});
        assert!(chain.evaluate(&get_input).await.unwrap());

        let post_input = serde_json::json!({"method": "POST", "url": "https://example.com"});
        assert!(!chain.evaluate(&post_input).await.unwrap());
    }
}
