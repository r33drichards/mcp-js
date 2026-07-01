//! Policy-gated file loading for the `run_js` tool.
//!
//! When a caller supplies a `file` path instead of inline `code`, the server
//! reads that file **from its own filesystem** and executes its contents.
//! Because this is a host-side read driven by caller input, it is OFF by
//! default and unlocked only by one of:
//!
//!   - `--allow-run-js-file` — allow reading any path the server process can
//!     access (the easy "allow all" switch), or
//!   - a `run_js_file` policy in `--policies-json` — a Rego/OPA chain decides,
//!     per path, which files may be read (e.g. restrict to a directory).
//!
//! The path is canonicalized (symlinks and `..` resolved) before policy
//! evaluation and reading, so a policy that authorizes by directory prefix
//! cannot be bypassed with `../` segments.

use std::sync::Arc;

use serde::Serialize;

use super::opa::PolicyChain;

/// How `run_js` file-path reads are authorized.
#[derive(Clone, Debug)]
pub enum RunJsFilePolicy {
    /// Allow reading any path the server process can access
    /// (`--allow-run-js-file`).
    AllowAll,
    /// Evaluate each path against a policy chain
    /// (`run_js_file` in `--policies-json`).
    Policy(Arc<PolicyChain>),
}

/// Input handed to a `run_js_file` policy for each read.
#[derive(Serialize)]
struct RunJsFilePolicyInput<'a> {
    /// Always `"read"` — the only operation on a script file.
    operation: &'a str,
    /// Canonicalized (symlink- and `..`-resolved) absolute path.
    path: &'a str,
    /// `X-MCP-*` headers from the MCP session, when available.
    #[serde(skip_serializing_if = "Option::is_none")]
    mcp_headers: Option<&'a serde_json::Value>,
}

impl RunJsFilePolicy {
    /// Authorize and read `path`, returning its contents as a UTF-8 string.
    ///
    /// Returns an error if the path cannot be canonicalized (e.g. it does not
    /// exist), if the policy denies the read, or if the file is not valid
    /// UTF-8.
    pub async fn read(
        &self,
        path: &str,
        mcp_headers: Option<&serde_json::Value>,
    ) -> Result<String, String> {
        // Canonicalize first so policies (and our own checks) see the real,
        // symlink-resolved path rather than caller-controlled `..` segments.
        // This also surfaces a clear not-found error before policy evaluation.
        let canonical = tokio::fs::canonicalize(path)
            .await
            .map_err(|e| format!("run_js file '{}': {}", path, e))?;
        let canonical_str = canonical.to_string_lossy().into_owned();

        if let RunJsFilePolicy::Policy(chain) = self {
            let input = serde_json::to_value(RunJsFilePolicyInput {
                operation: "read",
                path: &canonical_str,
                mcp_headers,
            })
            .map_err(|e| format!("run_js file policy input error: {}", e))?;

            let allowed = chain
                .evaluate(&input)
                .await
                .map_err(|e| format!("run_js file policy evaluation failed: {}", e))?;
            if !allowed {
                return Err(format!(
                    "run_js file '{}' denied by run_js_file policy",
                    canonical_str
                ));
            }
        }

        tokio::fs::read_to_string(&canonical)
            .await
            .map_err(|e| format!("run_js file '{}': {}", canonical_str, e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::opa::{EvalMode, LocalPolicyEvaluator, PolicyChain, PolicyEvaluatorKind};

    #[tokio::test]
    async fn test_allow_all_reads_any_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("script.js");
        std::fs::write(&path, "console.log(1 + 1);\n").unwrap();

        let policy = RunJsFilePolicy::AllowAll;
        let code = policy
            .read(path.to_str().unwrap(), None)
            .await
            .expect("allow-all should read the file");
        assert_eq!(code, "console.log(1 + 1);\n");
    }

    #[tokio::test]
    async fn test_missing_file_errors() {
        let policy = RunJsFilePolicy::AllowAll;
        let err = policy
            .read("/no/such/file/here.js", None)
            .await
            .expect_err("missing file should error");
        assert!(err.contains("/no/such/file/here.js"), "got: {err}");
    }

    fn rego_allow_under_dir(dir: &str) -> String {
        // Allow only paths beginning with `dir`.
        format!(
            r#"
package mcp.run_js_file

default allow = false

allow if {{
    startswith(input.path, "{}")
}}
"#,
            dir
        )
    }

    fn chain_from_rego(dir: &std::path::Path, rego: &str) -> Arc<PolicyChain> {
        let rego_path = dir.join("run_js_file.rego");
        std::fs::write(&rego_path, rego).unwrap();
        let eval =
            LocalPolicyEvaluator::from_file(&rego_path, "data.mcp.run_js_file.allow".to_string())
                .unwrap();
        Arc::new(PolicyChain::new(
            vec![PolicyEvaluatorKind::Local(eval)],
            EvalMode::All,
        ))
    }

    #[tokio::test]
    async fn test_policy_allows_path_in_directory() {
        let dir = tempfile::tempdir().unwrap();
        let allowed_dir = dir.path().join("allowed");
        std::fs::create_dir(&allowed_dir).unwrap();
        let script = allowed_dir.join("ok.js");
        std::fs::write(&script, "console.log('ok');\n").unwrap();

        // Canonicalize the directory for the rule so it matches the
        // canonicalized path the policy receives.
        let canonical_allowed = std::fs::canonicalize(&allowed_dir).unwrap();
        let chain = chain_from_rego(
            dir.path(),
            &rego_allow_under_dir(&canonical_allowed.to_string_lossy()),
        );

        let policy = RunJsFilePolicy::Policy(chain);
        let code = policy
            .read(script.to_str().unwrap(), None)
            .await
            .expect("policy should allow file under the allowed dir");
        assert_eq!(code, "console.log('ok');\n");
    }

    #[tokio::test]
    async fn test_policy_denies_path_outside_directory() {
        let dir = tempfile::tempdir().unwrap();
        let allowed_dir = dir.path().join("allowed");
        std::fs::create_dir(&allowed_dir).unwrap();
        let outside = dir.path().join("secret.js");
        std::fs::write(&outside, "console.log('secret');\n").unwrap();

        let canonical_allowed = std::fs::canonicalize(&allowed_dir).unwrap();
        let chain = chain_from_rego(
            dir.path(),
            &rego_allow_under_dir(&canonical_allowed.to_string_lossy()),
        );

        let policy = RunJsFilePolicy::Policy(chain);
        let err = policy
            .read(outside.to_str().unwrap(), None)
            .await
            .expect_err("policy should deny file outside the allowed dir");
        assert!(err.contains("denied by run_js_file policy"), "got: {err}");
    }
}
