use std::sync::Arc;

use deno_core::ModuleLoadOptions;
use deno_core::ModuleLoadReferrer;
use deno_core::ModuleLoadResponse;
use deno_core::ModuleLoader;
use deno_core::ModuleSource;
use deno_core::ModuleSourceCode;
use deno_core::ModuleSpecifier;
use deno_core::ModuleType;
use deno_core::ResolutionKind;
use deno_core::resolve_import;
use deno_core::FastString;
use deno_error::JsErrorBox;
use futures::FutureExt;
use serde::Serialize;

use super::opa::{OpaClient, PolicyChain};

/// Configuration for the module loader controlling external module access.
#[derive(Clone, Debug)]
pub struct ModuleLoaderConfig {
    /// When false, all external module imports (npm:, jsr:, URL) are rejected.
    pub allow_external: bool,
    /// Optional OPA client for auditing module imports before fetching.
    pub opa_client: Option<OpaClient>,
    /// OPA policy path for module auditing (e.g. "mcp/modules").
    pub opa_module_policy: Option<String>,
    /// Optional policy chain for module auditing (from `--policies-json`).
    pub policy_chain: Option<Arc<PolicyChain>>,
}

/// Input sent to OPA for module import auditing.
#[derive(Serialize)]
struct ModulePolicyInput {
    /// The original specifier as written in code (e.g. "npm:lodash-es@4.17.21").
    specifier: String,
    /// The type of specifier: "npm", "jsr", "url", or "relative".
    specifier_type: String,
    /// The resolved URL that will be fetched (e.g. "https://esm.sh/lodash-es@4.17.21").
    resolved_url: String,
    /// Parsed components of the resolved URL.
    url_parsed: ModuleUrlParsed,
}

#[derive(Serialize)]
struct ModuleUrlParsed {
    scheme: String,
    host: String,
    path: String,
}

/// Module loader that resolves `npm:`, `jsr:`, and URL imports by fetching
/// them from the network. NPM and JSR specifiers are rewritten to esm.sh
/// URLs so that packages are served as standard ES modules.
///
/// When `allow_external` is false, all external module imports are rejected
/// at resolution time. When an OPA module policy is configured, each module
/// is audited against the policy before being fetched from the network.
pub struct NetworkModuleLoader {
    client: reqwest::Client,
    config: ModuleLoaderConfig,
}

impl NetworkModuleLoader {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
            config: ModuleLoaderConfig {
                allow_external: true,
                opa_client: None,
                opa_module_policy: None,
                policy_chain: None,
            },
        }
    }

    pub fn with_config(config: ModuleLoaderConfig) -> Self {
        Self {
            client: reqwest::Client::new(),
            config,
        }
    }
}

impl ModuleLoader for NetworkModuleLoader {
    fn resolve(
        &self,
        specifier: &str,
        referrer: &str,
        _kind: ResolutionKind,
    ) -> Result<ModuleSpecifier, JsErrorBox> {
        // npm:cowsay@1.6.0 → https://esm.sh/cowsay@1.6.0
        if let Some(rest) = specifier.strip_prefix("npm:") {
            if !self.config.allow_external {
                return Err(JsErrorBox::generic(format!(
                    "External module imports are disabled. Cannot import npm package '{}'. \
                     Start the server with --allow-external-modules to enable.",
                    specifier
                )));
            }
            let url = format!("https://esm.sh/{}", rest);
            return ModuleSpecifier::parse(&url)
                .map_err(|e| JsErrorBox::generic(format!("Bad npm specifier '{}': {}", specifier, e)));
        }

        // jsr:@luca/cases@1.0.0 → https://esm.sh/jsr/@luca/cases@1.0.0
        if let Some(rest) = specifier.strip_prefix("jsr:") {
            if !self.config.allow_external {
                return Err(JsErrorBox::generic(format!(
                    "External module imports are disabled. Cannot import JSR package '{}'. \
                     Start the server with --allow-external-modules to enable.",
                    specifier
                )));
            }
            let url = format!("https://esm.sh/jsr/{}", rest);
            return ModuleSpecifier::parse(&url)
                .map_err(|e| JsErrorBox::generic(format!("Bad jsr specifier '{}': {}", specifier, e)));
        }

        // Absolute URLs pass through directly.
        if specifier.starts_with("https://") || specifier.starts_with("http://") {
            if !self.config.allow_external {
                return Err(JsErrorBox::generic(format!(
                    "External module imports are disabled. Cannot import URL module '{}'. \
                     Start the server with --allow-external-modules to enable.",
                    specifier
                )));
            }
            return ModuleSpecifier::parse(specifier)
                .map_err(|e| JsErrorBox::generic(format!("Bad URL '{}': {}", specifier, e)));
        }

        // Relative specifiers (./foo, ../bar) resolve against the referrer.
        resolve_import(specifier, referrer).map_err(JsErrorBox::from_err)
    }

    fn load(
        &self,
        module_specifier: &ModuleSpecifier,
        _maybe_referrer: Option<&ModuleLoadReferrer>,
        _options: ModuleLoadOptions,
    ) -> ModuleLoadResponse {
        let scheme = module_specifier.scheme();
        if scheme != "https" && scheme != "http" {
            return ModuleLoadResponse::Sync(Err(JsErrorBox::generic(
                format!(
                    "Cannot load module '{}': only https/http modules are supported",
                    module_specifier
                ),
            )));
        }

        let client = self.client.clone();
        let specifier = module_specifier.clone();
        let opa_client = self.config.opa_client.clone();
        let opa_policy = self.config.opa_module_policy.clone();
        let policy_chain = self.config.policy_chain.clone();
        let specifier_url_str = specifier.to_string();

        let fut = async move {
            // OPA module audit: check with policy before fetching
            if let (Some(opa), Some(policy_path)) = (&opa_client, &opa_policy) {
                let parsed = url::Url::parse(specifier_url_str.as_str()).ok();
                let url_parsed = parsed.as_ref().map(|p| ModuleUrlParsed {
                    scheme: p.scheme().to_string(),
                    host: p.host_str().unwrap_or("").to_string(),
                    path: p.path().to_string(),
                }).unwrap_or(ModuleUrlParsed {
                    scheme: String::new(),
                    host: String::new(),
                    path: String::new(),
                });

                // Determine specifier type from the resolved URL
                let spec_type = if specifier_url_str.contains("esm.sh/jsr/") {
                    "jsr"
                } else if specifier_url_str.contains("esm.sh/") {
                    "npm"
                } else {
                    "url"
                };

                let policy_input = ModulePolicyInput {
                    specifier: specifier_url_str.clone(),
                    specifier_type: spec_type.to_string(),
                    resolved_url: specifier_url_str.clone(),
                    url_parsed,
                };

                let allowed = opa
                    .evaluate(policy_path, &policy_input)
                    .await
                    .map_err(|e| JsErrorBox::generic(format!(
                        "OPA module policy check failed for '{}': {}",
                        specifier, e
                    )))?;

                if !allowed {
                    return Err(JsErrorBox::generic(format!(
                        "Module import denied by policy: '{}' is not allowed by the module policy",
                        specifier
                    )));
                }
            }

            // Evaluate policy chain if configured.
            if let Some(ref chain) = policy_chain {
                let parsed = url::Url::parse(specifier_url_str.as_str()).ok();
                let url_parsed = parsed.as_ref().map(|p| ModuleUrlParsed {
                    scheme: p.scheme().to_string(),
                    host: p.host_str().unwrap_or("").to_string(),
                    path: p.path().to_string(),
                }).unwrap_or(ModuleUrlParsed {
                    scheme: String::new(),
                    host: String::new(),
                    path: String::new(),
                });

                let spec_type = if specifier_url_str.contains("esm.sh/jsr/") {
                    "jsr"
                } else if specifier_url_str.contains("esm.sh/") {
                    "npm"
                } else {
                    "url"
                };

                let chain_input = ModulePolicyInput {
                    specifier: specifier_url_str.clone(),
                    specifier_type: spec_type.to_string(),
                    resolved_url: specifier_url_str.clone(),
                    url_parsed,
                };

                let input_value = serde_json::to_value(&chain_input)
                    .map_err(|e| JsErrorBox::generic(format!(
                        "Failed to serialize module policy input: {}", e
                    )))?;

                let allowed = chain
                    .evaluate(&input_value)
                    .await
                    .map_err(|e| JsErrorBox::generic(format!(
                        "Module policy chain check failed for '{}': {}",
                        specifier, e
                    )))?;

                if !allowed {
                    return Err(JsErrorBox::generic(format!(
                        "Module import denied by policy: '{}' is not allowed by the module policy",
                        specifier
                    )));
                }
            }

            let resp = client
                .get(specifier.as_str())
                .send()
                .await
                .map_err(|e| {
                    JsErrorBox::generic(format!(
                        "Failed to fetch module '{}': {}",
                        specifier, e
                    ))
                })?;

            if !resp.status().is_success() {
                return Err(JsErrorBox::generic(format!(
                    "Failed to fetch module '{}': HTTP {}",
                    specifier,
                    resp.status()
                )));
            }

            let final_url = resp.url().clone();
            let text = resp.text().await.map_err(|e| {
                JsErrorBox::generic(format!(
                    "Failed to read module '{}': {}",
                    specifier, e
                ))
            })?;

            // Strip TypeScript types for .ts/.tsx URLs.
            let url_path = final_url.path();
            let code = if url_path.ends_with(".ts") || url_path.ends_with(".tsx") {
                crate::engine::strip_typescript_types(&text).map_err(|e| {
                    JsErrorBox::generic(format!(
                        "Failed to transpile '{}': {}",
                        specifier, e
                    ))
                })?
            } else {
                text
            };

            // If the server redirected (e.g. esm.sh version resolution), record
            // the final URL so that relative imports within the module resolve
            // against the correct base.
            let source = if final_url.as_str() != specifier.as_str() {
                ModuleSource::new_with_redirect(
                    ModuleType::JavaScript,
                    ModuleSourceCode::String(FastString::from(code)),
                    &specifier,
                    &final_url,
                    None,
                )
            } else {
                ModuleSource::new(
                    ModuleType::JavaScript,
                    ModuleSourceCode::String(FastString::from(code)),
                    &specifier,
                    None,
                )
            };

            Ok(source)
        };

        ModuleLoadResponse::Async(fut.boxed_local())
    }
}
