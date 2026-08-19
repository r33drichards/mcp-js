use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

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

use super::opa::PolicyChain;

/// Per-request timeout for module fetches. Limits how long a single HTTP
/// request (DNS + connect + transfer) can take. Prevents hanging on
/// unreachable hosts or slow networks.
const MODULE_FETCH_TIMEOUT: Duration = Duration::from_secs(30);
/// Connect-phase timeout. Fails fast when the remote host is unreachable
/// (e.g. non-existent domain) without waiting for the full request timeout.
const MODULE_FETCH_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Configuration for the module loader controlling external module access.
#[derive(Clone, Debug)]
pub struct ModuleLoaderConfig {
    /// When false, all external module imports (npm:, jsr:, URL) are rejected.
    pub allow_external: bool,
    /// Optional policy chain for module auditing (from `--policies-json`).
    pub policy_chain: Option<Arc<PolicyChain>>,
    /// Explicit in-memory ES modules used by internal harnesses.
    pub virtual_modules: Option<Arc<HashMap<String, String>>>,
    /// Raw CommonJS modules available to `createRequire()` in virtual packages.
    pub virtual_commonjs_modules: Option<Arc<HashMap<String, String>>>,
    /// File URLs available to Node-compatible resolution without loading them.
    pub virtual_files: Option<Arc<HashSet<String>>>,
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
/// at resolution time. When a policy chain is configured, each module
/// is audited against the policy before being fetched from the network.
pub struct NetworkModuleLoader {
    client: reqwest::Client,
    config: ModuleLoaderConfig,
}

fn package_specifier_parts(specifier: &str) -> Option<(String, String)> {
    if specifier.starts_with('.') || specifier.starts_with('/') || specifier.contains(':') {
        return None;
    }
    let mut parts = specifier.split('/');
    let first = parts.next()?;
    let package = if first.starts_with('@') {
        format!("{}/{}", first, parts.next()?)
    } else {
        first.to_string()
    };
    let consumed = package.split('/').count();
    let subpath = specifier
        .split('/')
        .skip(consumed)
        .collect::<Vec<_>>()
        .join("/");
    Some((package, subpath))
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum PackageTarget {
    Resolved(String),
    Missing,
    Invalid,
}

fn package_error_module(code: &str, message: &str) -> ModuleSpecifier {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    code.hash(&mut hasher);
    message.hash(&mut hasher);
    let mut specifier = ModuleSpecifier::parse(&format!(
        "file:///__mcp_v8_node_errors__/{:016x}.mjs",
        hasher.finish()
    ))
    .unwrap();
    specifier
        .query_pairs_mut()
        .append_pair("code", code)
        .append_pair("message", message);
    specifier
}

fn package_target(
    value: &serde_json::Value,
    subpath: &str,
    conditions: &[&str],
) -> PackageTarget {
    match value {
        serde_json::Value::String(target) => {
            if !target.starts_with("./") {
                return PackageTarget::Invalid;
            }
            let resolved = target.replace('*', subpath);
            if resolved
                .trim_start_matches("./")
                .split('/')
                .filter_map(decode_percent_segment)
                .any(|segment| segment.eq_ignore_ascii_case("node_modules"))
            {
                return PackageTarget::Invalid;
            }
            let mut normalized = String::with_capacity(resolved.len());
            let mut previous_slash = false;
            for character in resolved.chars() {
                if character == '/' && previous_slash {
                    continue;
                }
                previous_slash = character == '/';
                normalized.push(character);
            }
            PackageTarget::Resolved(normalized)
        }
        serde_json::Value::Null => PackageTarget::Missing,
        serde_json::Value::Array(targets) => {
            let mut invalid = false;
            for target in targets {
                match package_target(target, subpath, conditions) {
                    resolved @ PackageTarget::Resolved(_) => return resolved,
                    PackageTarget::Invalid => invalid = true,
                    PackageTarget::Missing => {}
                }
            }
            if invalid {
                PackageTarget::Invalid
            } else {
                PackageTarget::Missing
            }
        }
        serde_json::Value::Object(targets) => {
            for (condition, target) in targets {
                if condition == "default" || conditions.contains(&condition.as_str()) {
                    let resolved = package_target(target, subpath, conditions);
                    if resolved != PackageTarget::Missing {
                        return resolved;
                    }
                }
            }
            PackageTarget::Missing
        }
        _ => PackageTarget::Invalid,
    }
}

fn package_exports_target(
    exports: &serde_json::Value,
    package_subpath: &str,
    conditions: &[&str],
) -> Result<PackageTarget, String> {
    let serde_json::Value::Object(exports_map) = exports else {
        return Ok(if package_subpath == "." {
            package_target(exports, "", conditions)
        } else {
            PackageTarget::Missing
        });
    };
    if exports_map
        .keys()
        .any(|key| key.parse::<u64>().is_ok() && key == &key.parse::<u64>().unwrap().to_string())
    {
        return Err("Invalid package config: \"exports\" cannot contain numeric property keys".into());
    }
    let subpath_keys = exports_map.keys().filter(|key| key.starts_with('.')).count();
    if subpath_keys > 0 && subpath_keys != exports_map.len() {
        return Err(
            "Invalid package config: \"exports\" cannot contain some keys starting with '.' and some not. The exports object must either be an object of package subpath keys or an object of main entry condition name keys only."
                .into(),
        );
    }
    if subpath_keys == 0 {
        return Ok(if package_subpath == "." {
            package_target(exports, "", conditions)
        } else {
            PackageTarget::Missing
        });
    }
    if let Some(target) = exports_map.get(package_subpath) {
        return Ok(package_target(target, "", conditions));
    }
    let mut patterns = exports_map
        .iter()
        .filter_map(|(key, target)| {
            let wildcard = key.find('*')?;
            let prefix = &key[..wildcard];
            let suffix = &key[wildcard + 1..];
            if package_subpath.len() <= prefix.len() + suffix.len() {
                return None;
            }
            package_subpath
                .strip_prefix(prefix)
                .and_then(|rest| rest.strip_suffix(suffix))
                .map(|matched| (prefix.len(), suffix.len(), matched, target))
        })
        .collect::<Vec<_>>();
    patterns.sort_by_key(|(prefix, suffix, _, _)| std::cmp::Reverse((*prefix, *suffix)));
    Ok(patterns
        .into_iter()
        .next()
        .map(|(_, _, matched, target)| package_target(target, matched, conditions))
        .unwrap_or(PackageTarget::Missing))
}

fn resolve_package_json(
    modules: &HashMap<String, String>,
    source: &str,
    package_json: &ModuleSpecifier,
    subpath: &str,
    conditions: &[&str],
) -> ModuleSpecifier {
    let package_data: serde_json::Value = match serde_json::from_str(source) {
        Ok(value) => value,
        Err(error) => {
            return package_error_module(
                "ERR_INVALID_PACKAGE_CONFIG",
                &format!("Invalid package config '{}': {}", package_json, error),
            );
        }
    };
    let package_subpath = if subpath.is_empty() {
        ".".to_string()
    } else {
        format!("./{subpath}")
    };
    let has_exports = package_data.get("exports").is_some();
    let target = match package_data.get("exports") {
        Some(exports) => match package_exports_target(exports, &package_subpath, conditions) {
            Ok(target) => target,
            Err(message) => {
                return package_error_module("ERR_INVALID_PACKAGE_CONFIG", &message);
            }
        },
        None if subpath.is_empty() => PackageTarget::Resolved(
            package_data
                .get("main")
                .and_then(|value| value.as_str())
                .unwrap_or("./index.js")
                .to_string(),
        ),
        None => PackageTarget::Resolved(format!("./{subpath}")),
    };
    let target = match target {
        PackageTarget::Resolved(target) => target,
        PackageTarget::Invalid => {
            return package_error_module(
                "ERR_INVALID_PACKAGE_TARGET",
                &format!(
                    "Invalid \"exports\" target for '{}'; targets must start with './'",
                    package_subpath
                ),
            );
        }
        PackageTarget::Missing => {
            let message = if package_subpath == "." {
                format!("No \"exports\" main defined in {package_json}")
            } else {
                format!("Package subpath '{}' is not defined by exports", package_subpath)
            };
            return package_error_module("ERR_PACKAGE_PATH_NOT_EXPORTED", &message);
        }
    };
    let target_url = match package_json.join(&target) {
        Ok(target_url) => target_url,
        Err(error) => {
            return package_error_module("ERR_INVALID_PACKAGE_TARGET", &error.to_string());
        }
    };
    if modules.contains_key(target_url.as_str()) {
        return target_url;
    }
    if !has_exports {
        for suffix in [".js", ".json"] {
            let candidate = format!("{}{suffix}", target_url.as_str());
            if modules.contains_key(&candidate) {
                return ModuleSpecifier::parse(&candidate).unwrap();
            }
        }
        let candidate = format!("{}/index.js", target_url.as_str().trim_end_matches('/'));
        if modules.contains_key(&candidate) {
            return ModuleSpecifier::parse(&candidate).unwrap();
        }
    }
    let directory_prefix = format!("{}/", target_url.as_str().trim_end_matches('/'));
    if modules.keys().any(|specifier| specifier.starts_with(&directory_prefix)) {
        return package_error_module(
            "ERR_UNSUPPORTED_DIR_IMPORT",
            &format!("Directory import '{}' is not supported", target_url.path()),
        );
    }
    package_error_module(
        "ERR_MODULE_NOT_FOUND",
        &format!("Cannot find module '{}'", target_url.path()),
    )
}

fn decode_percent_segment(segment: &str) -> Option<String> {
    let bytes = segment.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len() {
                return None;
            }
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok()?;
            decoded.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

fn invalid_package_subpath(subpath: &str) -> bool {
    subpath.split('/').any(|segment| {
        let Some(decoded) = decode_percent_segment(segment) else {
            return true;
        };
        let decoded = decoded.to_ascii_lowercase();
        decoded == "."
            || decoded == ".."
            || decoded == "node_modules"
            || decoded.contains('/')
            || decoded.contains('\\')
    })
}

fn resolve_virtual_package(
    modules: &HashMap<String, String>,
    specifier: &str,
    referrer: &str,
    conditions: &[&str],
) -> Option<Result<ModuleSpecifier, JsErrorBox>> {
    let (package, subpath) = package_specifier_parts(specifier)?;
    if !subpath.is_empty() && invalid_package_subpath(&subpath) {
        let package_subpath = format!("./{subpath}");
        return Some(Ok(package_error_module(
            "ERR_INVALID_MODULE_SPECIFIER",
            &format!(
                "Invalid module '{}' is not a valid match in pattern '{}'",
                specifier, package_subpath
            ),
        )));
    }
    let referrer = ModuleSpecifier::parse(referrer).ok()?;
    if referrer.scheme() != "file" {
        return None;
    }
    let mut directory = referrer.join(".").ok()?;
    loop {
        let self_package_json = directory.join("package.json").ok()?;
        if let Some(source) = modules.get(self_package_json.as_str()) {
            let package_data: serde_json::Value = match serde_json::from_str(source) {
                Ok(value) => value,
                Err(error) => {
                    return Some(Err(JsErrorBox::generic(format!(
                        "Invalid package config '{}': {}",
                        self_package_json, error
                    ))));
                }
            };
            if package_data.get("name").and_then(|value| value.as_str()) == Some(package.as_str()) {
                return Some(Ok(resolve_package_json(
                    modules,
                    source,
                    &self_package_json,
                    &subpath,
                    conditions,
                )));
            }
        }
        let package_json = directory
            .join(&format!("node_modules/{package}/package.json"))
            .ok()?;
        if let Some(source) = modules.get(package_json.as_str()) {
            return Some(Ok(resolve_package_json(
                modules,
                source,
                &package_json,
                &subpath,
                conditions,
            )));
        }
        let parent = directory.join("..").ok()?;
        if parent == directory {
            return None;
        }
        directory = parent;
    }
}

impl NetworkModuleLoader {
    fn build_client() -> reqwest::Client {
        reqwest::Client::builder()
            .connect_timeout(MODULE_FETCH_CONNECT_TIMEOUT)
            .timeout(MODULE_FETCH_TIMEOUT)
            .build()
            .expect("failed to build reqwest client")
    }

    pub fn new() -> Self {
        Self {
            client: Self::build_client(),
            config: ModuleLoaderConfig {
                allow_external: true,
                policy_chain: None,
                virtual_modules: None,
                virtual_commonjs_modules: None,
                virtual_files: None,
            },
        }
    }

    pub fn with_config(config: ModuleLoaderConfig) -> Self {
        Self {
            client: Self::build_client(),
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
        if self.config.virtual_modules.as_ref().is_some_and(|modules| modules.contains_key(specifier)) {
            return ModuleSpecifier::parse(specifier).map_err(|e| {
                JsErrorBox::generic(format!("Bad virtual module specifier '{}': {}", specifier, e))
            });
        }

        // node:path → served from the embedded Node compat registry.
        // Bare builtin names (e.g. "path") also resolve here, matching Node.
        let node_name = specifier.strip_prefix("node:").unwrap_or(specifier);
        let internal_allowed =
            !node_name.starts_with("internal/") || self.config.virtual_files.is_some();
        if internal_allowed && super::node_compat::resolve_submodule(node_name).is_some() {
            return ModuleSpecifier::parse(&format!("node:{}", node_name)).map_err(|e| {
                JsErrorBox::generic(format!("Bad node specifier '{}': {}", specifier, e))
            });
        }

        if let Some(modules) = self.config.virtual_modules.as_deref() {
            if let Some(resolved) =
                resolve_virtual_package(modules, specifier, referrer, &["import", "node", "default"])
            {
                return resolved;
            }
        }

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
        // Check for "https:" / "http:" (not just "https://" / "http://") so that
        // malformed specifiers like "https:1/es" are caught here rather than
        // falling through to resolve_import which would parse them as valid URLs.
        if specifier.starts_with("https:") || specifier.starts_with("http:") {
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
        if let Some(source) = self.config.virtual_modules.as_ref()
            .and_then(|modules| modules.get(module_specifier.as_str()))
        {
            let module_type = if module_specifier.path().ends_with(".json") {
                ModuleType::Json
            } else {
                ModuleType::JavaScript
            };
            return ModuleLoadResponse::Sync(Ok(ModuleSource::new(
                module_type,
                ModuleSourceCode::String(FastString::from(source.clone())),
                module_specifier,
                None,
            )));
        }

        let scheme = module_specifier.scheme();
        if module_specifier.path().starts_with("/__mcp_v8_node_errors__/") {
            let mut code = "ERR_MODULE_NOT_FOUND".to_string();
            let mut message = "Module resolution failed".to_string();
            for (key, value) in module_specifier.query_pairs() {
                match key.as_ref() {
                    "code" => code = value.into_owned(),
                    "message" => message = value.into_owned(),
                    _ => {}
                }
            }
            let source = format!(
                "const error=new Error({});error.code={};throw error;",
                serde_json::to_string(&message).unwrap(),
                serde_json::to_string(&code).unwrap(),
            );
            return ModuleLoadResponse::Sync(Ok(ModuleSource::new(
                ModuleType::JavaScript,
                ModuleSourceCode::String(FastString::from(source)),
                module_specifier,
                None,
            )));
        }
        if scheme == "node" {
            let name = module_specifier.path();
            if name.starts_with("internal/") && self.config.virtual_files.is_none() {
                return ModuleLoadResponse::Sync(Err(JsErrorBox::generic(format!(
                    "Unknown node builtin module: '{}'",
                    name
                ))));
            }
            return match super::node_compat::resolve_submodule(name) {
                Some(mut source) => {
                    if name == "module" {
                        if self.config.virtual_files.is_some() {
                            source = format!(
                                "import __mcpV8InternalEsmResolve from 'node:internal/modules/esm/resolve';\n{}",
                                source,
                            );
                        }
                        let empty_commonjs = HashMap::new();
                        let commonjs = self
                            .config
                            .virtual_commonjs_modules
                            .as_deref()
                            .unwrap_or(&empty_commonjs);
                        let package_json = self
                            .config
                            .virtual_modules
                            .as_deref()
                            .map(|modules| {
                                modules
                                    .iter()
                                    .filter(|(specifier, _)| specifier.ends_with("/package.json"))
                                    .map(|(specifier, source)| (specifier.clone(), source.clone()))
                                    .collect::<HashMap<_, _>>()
                            })
                            .unwrap_or_default();
                        source = format!(
                            "globalThis.__mcpV8VirtualCommonJsModules={};\n\
                             globalThis.__mcpV8VirtualPackageJson={};\n{}",
                            serde_json::to_string(commonjs).unwrap(),
                            serde_json::to_string(&package_json).unwrap(),
                            source,
                        );
                    } else if name == "internal/modules/esm/resolve" {
                        let files = self.config.virtual_files.as_deref().unwrap();
                        source = format!(
                            "{}\nconst __mcpV8VirtualFiles=new Set({});",
                            source,
                            serde_json::to_string(files).unwrap(),
                        );
                    }
                    ModuleLoadResponse::Sync(Ok(ModuleSource::new(
                        ModuleType::JavaScript,
                        ModuleSourceCode::String(FastString::from(source)),
                        module_specifier,
                        None,
                    )))
                }
                None => ModuleLoadResponse::Sync(Err(JsErrorBox::generic(format!(
                    "Unknown node builtin module: '{}'",
                    name
                )))),
            };
        }
        if scheme != "https" && scheme != "http" {
            return ModuleLoadResponse::Sync(Err(JsErrorBox::generic(
                format!(
                    "Cannot load module '{}': only https/http modules are supported",
                    module_specifier
                ),
            )));
        }

        // Defense-in-depth: block network requests even if resolve() let something through.
        if !self.config.allow_external {
            return ModuleLoadResponse::Sync(Err(JsErrorBox::generic(format!(
                "External module imports are disabled. Cannot import URL module '{}'. \
                 Start the server with --allow-external-modules to enable.",
                module_specifier
            ))));
        }

        let client = self.client.clone();
        let specifier = module_specifier.clone();
        let policy_chain = self.config.policy_chain.clone();
        let specifier_url_str = specifier.to_string();

        let fut = async move {
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
