use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use deno_core::FastString;
use deno_core::ModuleLoadOptions;
use deno_core::ModuleLoadReferrer;
use deno_core::ModuleLoadResponse;
use deno_core::ModuleLoader;
use deno_core::ModuleSource;
use deno_core::ModuleSourceCode;
use deno_core::ModuleSpecifier;
use deno_core::ModuleType;
use deno_core::RequestedModuleType;
use deno_core::ResolutionKind;
use deno_core::resolve_import;
use deno_error::{AdditionalProperties, JsErrorBox, JsErrorClass, PropertyValue};
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
const FILE_ROOT_PREFIX: &str = "mcp-v8:file-root:";
const FILE_MAP_PREFIX: &str = "mcp-v8:file-map:";
const PACKAGE_MAP_PREFIX: &str = "mcp-v8:package-map:";

#[derive(Debug)]
struct NodeModuleError {
    class: &'static str,
    code: &'static str,
    message: String,
}

impl std::fmt::Display for NodeModuleError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for NodeModuleError {}

impl JsErrorClass for NodeModuleError {
    fn get_class(&self) -> Cow<'static, str> {
        Cow::Borrowed(self.class)
    }

    fn get_message(&self) -> Cow<'static, str> {
        Cow::Owned(self.message.clone())
    }

    fn get_additional_properties(&self) -> AdditionalProperties {
        Box::new(std::iter::once((
            Cow::Borrowed("code"),
            PropertyValue::from(self.code),
        )))
    }

    fn get_ref(&self) -> &(dyn std::error::Error + Send + Sync + 'static) {
        self
    }
}

fn node_module_error(class: &'static str, code: &'static str, message: String) -> JsErrorBox {
    JsErrorBox::from_err(NodeModuleError {
        class,
        code,
        message,
    })
}

fn import_attribute_error(
    module_specifier: &ModuleSpecifier,
    format: &str,
    requested: &RequestedModuleType,
) -> Option<NodeModuleError> {
    match requested {
        RequestedModuleType::None if format == "json" => Some(NodeModuleError {
            class: "TypeError",
            code: "ERR_IMPORT_ATTRIBUTE_MISSING",
            message: format!(
                "Module \"{}\" needs an import attribute of \"type: json\"",
                module_specifier
            ),
        }),
        RequestedModuleType::None => None,
        RequestedModuleType::Json if format == "json" => None,
        RequestedModuleType::Json => Some(NodeModuleError {
            class: "TypeError",
            code: "ERR_IMPORT_ATTRIBUTE_TYPE_INCOMPATIBLE",
            message: format!("Module \"{}\" is not of type \"json\"", module_specifier),
        }),
        requested => Some(NodeModuleError {
            class: "TypeError",
            code: "ERR_IMPORT_ATTRIBUTE_UNSUPPORTED",
            message: format!(
                "Import attribute \"type\" with value \"{}\" is not supported in {}",
                requested.as_str().unwrap_or(""),
                module_specifier
            ),
        }),
    }
}

fn decode_data_url(module_specifier: &ModuleSpecifier) -> Result<(String, Vec<u8>), JsErrorBox> {
    let data_url = data_url::DataUrl::process(module_specifier.as_str()).map_err(|_| {
        node_module_error(
            "TypeError",
            "ERR_INVALID_URL",
            format!("Invalid URL: {}", module_specifier),
        )
    })?;
    let mime = format!(
        "{}/{}",
        data_url.mime_type().type_,
        data_url.mime_type().subtype
    );
    let (body, _) = data_url.decode_to_vec().map_err(|_| {
        node_module_error(
            "TypeError",
            "ERR_INVALID_URL",
            format!("Invalid URL: {}", module_specifier),
        )
    })?;
    Ok((mime, body))
}

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

pub fn virtual_file_mapping(specifier: &ModuleSpecifier, path: &Path) -> String {
    format!(
        "{FILE_MAP_PREFIX}{}",
        serde_json::to_string(&(specifier.as_str(), path)).unwrap()
    )
}

fn allowed_file_path(config: &ModuleLoaderConfig, specifier: &ModuleSpecifier) -> Option<PathBuf> {
    let roots = config.virtual_files.as_deref()?;
    for marker in roots {
        let Some(mapping) = marker.strip_prefix(FILE_MAP_PREFIX) else {
            continue;
        };
        let Ok((virtual_specifier, host_path)) =
            serde_json::from_str::<(String, PathBuf)>(mapping)
        else {
            continue;
        };
        if virtual_specifier == specifier.as_str() {
            return std::fs::canonicalize(host_path).ok();
        }
    }
    let path = specifier.to_file_path().ok()?;
    for marker in roots {
        let Some(root) = marker.strip_prefix(FILE_ROOT_PREFIX) else {
            continue;
        };
        let root = ModuleSpecifier::parse(root).ok()?.to_file_path().ok()?;
        if !path.starts_with(&root) {
            continue;
        }
        return match std::fs::canonicalize(&path) {
            Ok(canonical) if canonical.starts_with(&root) => Some(canonical),
            Ok(_) => None,
            Err(_) => Some(path),
        };
    }
    None
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


#[derive(Clone, Debug, Eq, PartialEq)]
struct PackageWarning {
    code: &'static str,
    message: String,
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

fn node_error_module_source(code: &str, message: &str) -> String {
    format!(
        "export default undefined;const error=new Error({message});error.code={code};try{{if(typeof error.stack==='string')error.stack+='\\n  code: '+{quoted_code};}}catch{{}}throw error;",
        message = serde_json::to_string(&message).unwrap(),
        code = serde_json::to_string(&code).unwrap(),
        quoted_code = serde_json::to_string(&format!("'{code}'")).unwrap(),
    )
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

fn node_display_path(specifier: &ModuleSpecifier) -> String {
    if specifier.scheme() == "file" {
        specifier.path().to_owned()
    } else {
        specifier.to_string()
    }
}

fn invalid_package_config_message(
    package_json: &ModuleSpecifier,
    package: &str,
    referrer: &ModuleSpecifier,
    detail: &str,
) -> String {
    format!(
        "Invalid package config {} while importing {:?} from {}. {}",
        node_display_path(package_json),
        package,
        node_display_path(referrer),
        detail,
    )
}

fn mark_package_warning(
    mut specifier: ModuleSpecifier,
    warning: Option<&PackageWarning>,
) -> ModuleSpecifier {
    if let Some(warning) = warning {
        specifier
            .query_pairs_mut()
            .append_pair("__mcp_v8_warning_code", warning.code)
            .append_pair("__mcp_v8_warning_message", &warning.message);
    }
    specifier
}

fn package_main_warning(
    package_json: &ModuleSpecifier,
    referrer: &ModuleSpecifier,
    main: Option<&str>,
    resolved: &ModuleSpecifier,
) -> PackageWarning {
    let package_root = package_json.join("./").unwrap();
    let message = match main.filter(|value| !value.is_empty()) {
        Some(main) => {
            let resolved_name = resolved.path().rsplit('/').next().unwrap_or(main);
            format!(
                "Package {} has a \"main\" field set to {main:?}, excluding the full filename and extension to the resolved file at {resolved_name:?}, imported from {}.\n Automatic extension resolution of the \"main\" field is deprecated for ES modules.",
                node_display_path(&package_root),
                node_display_path(referrer),
            )
        }
        None => format!(
            "No \"main\" or \"exports\" field defined in the package.json for {} resolving the main entry point \"index.js\", imported from {}.\nDefault \"index\" lookups for the main are deprecated for ES modules.",
            node_display_path(&package_root),
            node_display_path(referrer),
        ),
    };
    PackageWarning {
        code: "DEP0151",
        message,
    }
}

fn resolve_package_json(
    modules: &HashMap<String, String>,
    source: &str,
    package_json: &ModuleSpecifier,
    package: &str,
    referrer: &ModuleSpecifier,
    subpath: &str,
    conditions: &[&str],
) -> ModuleSpecifier {
    let package_data: serde_json::Value = match serde_json::from_str(source) {
        Ok(value) => value,
        Err(error) => {
            return package_error_module(
                "ERR_INVALID_PACKAGE_CONFIG",
                &invalid_package_config_message(
                    package_json,
                    package,
                    referrer,
                    &error.to_string(),
                ),
            );
        }
    };
    let package_subpath = if subpath.is_empty() {
        ".".to_string()
    } else {
        format!("./{subpath}")
    };
    let has_exports = package_data.get("exports").is_some();
    let main = package_data.get("main").and_then(|value| value.as_str());
    let warn_dep0151 = !has_exports
        && main
            .filter(|value| !value.is_empty())
            .is_none_or(|value| Path::new(value).extension().is_none());
    let target = match package_data.get("exports") {
        Some(exports) => match package_exports_target(exports, &package_subpath, conditions) {
            Ok(target) => target,
            Err(message) => {
                return package_error_module("ERR_INVALID_PACKAGE_CONFIG", &message);
            }
        },
        None if subpath.is_empty() => PackageTarget::Resolved(
            main.filter(|value| !value.is_empty())
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
        let warning = warn_dep0151
            .then(|| package_main_warning(package_json, referrer, main, &target_url));
        return mark_package_warning(target_url, warning.as_ref());
    }
    if !has_exports {
        for suffix in [".js", ".json"] {
            let candidate = format!("{}{suffix}", target_url.as_str());
            if modules.contains_key(&candidate) {
                let candidate = ModuleSpecifier::parse(&candidate).unwrap();
                let warning = warn_dep0151
                    .then(|| package_main_warning(package_json, referrer, main, &candidate));
                return mark_package_warning(candidate, warning.as_ref());
            }
        }
        let candidate = format!("{}/index.js", target_url.as_str().trim_end_matches('/'));
        if modules.contains_key(&candidate) {
            let candidate = ModuleSpecifier::parse(&candidate).unwrap();
            let warning = warn_dep0151
                .then(|| package_main_warning(package_json, referrer, main, &candidate));
            return mark_package_warning(candidate, warning.as_ref());
        }
        if subpath.is_empty() {
            let package_root = package_json.join("./").unwrap();
            for name in ["index.js", "index.json"] {
                let candidate = package_root.join(name).unwrap();
                if modules.contains_key(candidate.as_str()) {
                    let warning = warn_dep0151
                        .then(|| package_main_warning(package_json, referrer, main, &candidate));
                    return mark_package_warning(candidate, warning.as_ref());
                }
            }
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

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct VirtualPackageMapPackage {
    url: String,
    dependencies: HashMap<String, String>,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
struct VirtualPackageMap {
    packages: HashMap<String, VirtualPackageMapPackage>,
}

pub fn virtual_package_map(value: &serde_json::Value) -> String {
    format!("{PACKAGE_MAP_PREFIX}{value}")
}

fn configured_package_map(config: &ModuleLoaderConfig) -> Option<VirtualPackageMap> {
    config.virtual_files.as_deref()?.iter().find_map(|marker| {
        marker
            .strip_prefix(PACKAGE_MAP_PREFIX)
            .and_then(|value| serde_json::from_str(value).ok())
    })
}

fn resolve_package_map(
    config: &ModuleLoaderConfig,
    modules: &HashMap<String, String>,
    specifier: &str,
    referrer: &str,
    conditions: &[&str],
) -> Option<Result<ModuleSpecifier, JsErrorBox>> {
    let (package, subpath) = package_specifier_parts(specifier)?;
    let package_map = configured_package_map(config)?;
    let referrer = ModuleSpecifier::parse(referrer).ok()?;
    let Some(owner) = package_map
        .packages
        .values()
        .filter(|entry| referrer.as_str().starts_with(&entry.url))
        .max_by_key(|entry| entry.url.len())
    else {
        return Some(Ok(package_error_module(
            "ERR_PACKAGE_MAP_EXTERNAL_FILE",
            &format!("File outside package map scope: {referrer}"),
        )));
    };
    let Some(target_key) = owner.dependencies.get(&package) else {
        return Some(Ok(package_error_module(
            "ERR_MODULE_NOT_FOUND",
            &format!("Cannot find package '{package}' imported from {referrer}"),
        )));
    };
    let Some(target) = package_map.packages.get(target_key) else {
        return Some(Ok(package_error_module(
            "ERR_PACKAGE_MAP_KEY_NOT_FOUND",
            &format!("Package map key '{target_key}' was not found"),
        )));
    };
    let package_root = ModuleSpecifier::parse(&target.url).ok()?;
    let package_json = package_root.join("package.json").ok()?;
    if let Some(source) = modules.get(package_json.as_str()) {
        return Some(Ok(resolve_package_json(
            modules,
            source,
            &package_json,
            &package,
            &referrer,
            &subpath,
            conditions,
        )));
    }
    let request = if subpath.is_empty() {
        "index"
    } else {
        &subpath
    };
    let target_url = package_root.join(request).ok()?;
    for candidate in [
        target_url.to_string(),
        format!("{}.js", target_url),
        format!("{}.cjs", target_url),
        format!("{}/index.js", target_url.as_str().trim_end_matches('/')),
        format!("{}/index.cjs", target_url.as_str().trim_end_matches('/')),
    ] {
        if modules.contains_key(&candidate) {
            return Some(ModuleSpecifier::parse(&candidate).map_err(JsErrorBox::from_err));
        }
    }
    Some(Ok(package_error_module(
        "ERR_MODULE_NOT_FOUND",
        &format!("Cannot find module '{}'", target_url.path()),
    )))
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
                    return Some(Ok(package_error_module(
                        "ERR_INVALID_PACKAGE_CONFIG",
                        &invalid_package_config_message(
                            &self_package_json,
                            &package,
                            &referrer,
                            &error.to_string(),
                        ),
                    )));
                }
            };
            if package_data.get("name").and_then(|value| value.as_str()) == Some(package.as_str()) {
                return Some(Ok(resolve_package_json(
                    modules,
                    source,
                    &self_package_json,
                    &package,
                    &referrer,
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
                &package,
                &referrer,
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
            if let Some(resolved) = resolve_package_map(
                &self.config,
                modules,
                specifier,
                referrer,
                &["import", "node", "default"],
            ) {
                return resolved;
            }
            if let Some(resolved) =
                resolve_virtual_package(modules, specifier, referrer, &["import", "node", "default"])
            {
                return resolved;
            }
            if let Some((package, _)) = package_specifier_parts(specifier) {
                return Ok(package_error_module(
                    "ERR_MODULE_NOT_FOUND",
                    &format!("Cannot find package '{package}' imported from {referrer}"),
                ));
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
        options: ModuleLoadOptions,
    ) -> ModuleLoadResponse {
        let virtual_source = self.config.virtual_modules.as_ref().and_then(|modules| {
            modules.get(module_specifier.as_str()).cloned().or_else(|| {
                if module_specifier.scheme() != "file" {
                    return None;
                }
                let path = module_specifier.to_file_path().ok()?;
                let normalized = ModuleSpecifier::from_file_path(path).ok()?;
                modules.get(normalized.as_str()).cloned()
            })
        });
        if let Some(mut source) = virtual_source {
            let mut warning_code = None;
            let mut warning_message = None;
            for (key, value) in module_specifier.query_pairs() {
                match key.as_ref() {
                    "__mcp_v8_warning_code" => warning_code = Some(value.into_owned()),
                    "__mcp_v8_warning_message" => warning_message = Some(value.into_owned()),
                    _ => {}
                }
            }
            if let (Some(code), Some(message)) = (warning_code, warning_message) {
                source = format!(
                    "import __mcpV8WarningProcess from 'node:process';\n__mcpV8WarningProcess.emitWarning({message}, 'DeprecationWarning', {code});\nawait new Promise((resolve) => __mcpV8WarningProcess.nextTick(resolve));\n{source}",
                    message = serde_json::to_string(&message).unwrap(),
                    code = serde_json::to_string(&code).unwrap(),
                );
            }
            let (format, module_type) = if module_specifier.path().ends_with(".json") {
                ("json", ModuleType::Json)
            } else {
                ("module", ModuleType::JavaScript)
            };
            if let Some(error) = import_attribute_error(
                module_specifier,
                format,
                &options.requested_module_type,
            ) {
                return ModuleLoadResponse::Sync(Err(JsErrorBox::from_err(error)));
            }
            return ModuleLoadResponse::Sync(Ok(ModuleSource::new(
                module_type,
                ModuleSourceCode::String(FastString::from(source)),
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
            let source = node_error_module_source(&code, &message);
            return ModuleLoadResponse::Sync(Ok(ModuleSource::new(
                ModuleType::JavaScript,
                ModuleSourceCode::String(FastString::from(source)),
                module_specifier,
                None,
            )));
        }
        if scheme == "node" {
            if let Some(error) = import_attribute_error(
                module_specifier,
                "module",
                &options.requested_module_type,
            ) {
                return ModuleLoadResponse::Sync(Err(JsErrorBox::from_err(error)));
            }
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
                        let package_map = serde_json::to_string(&configured_package_map(&self.config)).unwrap();
                        source = format!(
                            "globalThis.__mcpV8VirtualCommonJsModules={};\n\
                             globalThis.__mcpV8VirtualPackageJson={};\n\
                             globalThis.__mcpV8PackageMap=JSON.parse({});\n{}",
                            serde_json::to_string(commonjs).unwrap(),
                            serde_json::to_string(&package_json).unwrap(),
                            serde_json::to_string(&package_map).unwrap(),
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
        if scheme == "file"
            && self
                .config
                .virtual_files
                .as_deref()
                .is_some_and(|files| files.contains(module_specifier.as_str()))
            && let Some(extension) = Path::new(module_specifier.path())
                .extension()
                .and_then(|value| value.to_str())
            && !matches!(extension, "js" | "mjs" | "cjs" | "json" | "wasm")
        {
            return ModuleLoadResponse::Sync(Err(node_module_error(
                "TypeError",
                "ERR_UNKNOWN_FILE_EXTENSION",
                format!(
                    "Unknown file extension '.{extension}' for {}",
                    module_specifier.path()
                ),
            )));
        }
        if scheme == "file"
            && let Some(path) = allowed_file_path(&self.config, module_specifier)
        {
            let body = match std::fs::read(&path) {
                Ok(body) => body,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    return ModuleLoadResponse::Sync(Err(node_module_error(
                        "Error",
                        "ERR_MODULE_NOT_FOUND",
                        format!("Cannot find module '{}'", module_specifier),
                    )));
                }
                Err(error) => return ModuleLoadResponse::Sync(Err(JsErrorBox::from_err(error))),
            };
            let (format, module_type) = match path.extension().and_then(|value| value.to_str()) {
                Some("json") => ("json", ModuleType::Json),
                Some("wasm") => ("wasm", ModuleType::Wasm),
                _ => ("module", ModuleType::JavaScript),
            };
            if let Some(error) = import_attribute_error(
                module_specifier,
                format,
                &options.requested_module_type,
            ) {
                return ModuleLoadResponse::Sync(Err(JsErrorBox::from_err(error)));
            }
            let source = match module_type {
                ModuleType::Wasm => ModuleSourceCode::Bytes(body.into_boxed_slice().into()),
                _ => ModuleSourceCode::String(FastString::from(
                    String::from_utf8_lossy(&body).into_owned(),
                )),
            };
            return ModuleLoadResponse::Sync(Ok(ModuleSource::new(
                module_type,
                source,
                module_specifier,
                None,
            )));
        }
        if scheme == "data" {
            let (mime, body) = match decode_data_url(module_specifier) {
                Ok(data) => data,
                Err(error) => return ModuleLoadResponse::Sync(Err(error)),
            };
            let format = if mime.eq_ignore_ascii_case("application/json") {
                "json"
            } else if mime.eq_ignore_ascii_case("application/wasm") {
                "wasm"
            } else {
                let normalized = mime.trim().to_ascii_lowercase();
                let javascript = normalized == "text/javascript"
                    || normalized == "application/javascript"
                    || normalized == "text/javascript;charset=utf-8"
                    || normalized == "text/javascript;charset=utf8"
                    || normalized == "application/javascript;charset=utf-8"
                    || normalized == "application/javascript;charset=utf8";
                if javascript { "module" } else { "" }
            };
            if format.is_empty() {
                return ModuleLoadResponse::Sync(Err(JsErrorBox::from_err(
                    NodeModuleError {
                        class: "RangeError",
                        code: "ERR_UNKNOWN_MODULE_FORMAT",
                        message: format!(
                            "Unknown module format: {} for URL {}",
                            mime, module_specifier
                        ),
                    },
                )));
            }
            if let Some(error) = import_attribute_error(
                module_specifier,
                format,
                &options.requested_module_type,
            ) {
                return ModuleLoadResponse::Sync(Err(JsErrorBox::from_err(error)));
            }
            let module_type = match format {
                "json" => ModuleType::Json,
                "wasm" => ModuleType::Wasm,
                _ => ModuleType::JavaScript,
            };
            let source = match module_type {
                ModuleType::Wasm => ModuleSourceCode::Bytes(body.into_boxed_slice().into()),
                _ => ModuleSourceCode::String(FastString::from(
                    String::from_utf8_lossy(&body).into_owned(),
                )),
            };
            return ModuleLoadResponse::Sync(Ok(ModuleSource::new(
                module_type,
                source,
                module_specifier,
                None,
            )));
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
        if let Some(error) = import_attribute_error(
            module_specifier,
            "module",
            &options.requested_module_type,
        ) {
            return ModuleLoadResponse::Sync(Err(JsErrorBox::from_err(error)));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_main_falls_back_to_package_index() {
        let package_json =
            ModuleSpecifier::parse("file:///app/node_modules/deep-fail/package.json").unwrap();
        let referrer = ModuleSpecifier::parse("file:///app/main.mjs").unwrap();
        let modules = HashMap::from([(
            "file:///app/node_modules/deep-fail/index.js".to_owned(),
            "module.exports = {};".to_owned(),
        )]);

        let resolved = resolve_package_json(
            &modules,
            r#"{"main":"index.mjs"}"#,
            &package_json,
            "deep-fail",
            &referrer,
            "",
            &["import", "node", "default"],
        );

        assert_eq!(
            resolved.as_str(),
            "file:///app/node_modules/deep-fail/index.js"
        );
    }

    #[test]
    fn package_main_resolution_marks_dep0151_message() {
        let package_json =
            ModuleSpecifier::parse("file:///app/node_modules/no_exports/package.json").unwrap();
        let referrer = ModuleSpecifier::parse("file:///app/main.mjs").unwrap();
        let modules = HashMap::from([(
            "file:///app/node_modules/no_exports/index.js".to_owned(),
            "export default 'index';".to_owned(),
        )]);

        let resolved = resolve_package_json(
            &modules,
            r#"{"type":"module"}"#,
            &package_json,
            "no_exports",
            &referrer,
            "",
            &["import", "node", "default"],
        );
        let query = resolved.query_pairs().collect::<HashMap<_, _>>();

        assert_eq!(query.get("__mcp_v8_warning_code").unwrap(), "DEP0151");
        assert!(
            query
                .get("__mcp_v8_warning_message")
                .unwrap()
                .contains("no_exports")
        );
    }

    #[test]
    fn package_map_rejects_referrers_outside_mapped_packages() {
        let package_map = serde_json::json!({
            "packages": {
                "root": {
                    "url": "file:///app/root/",
                    "dependencies": { "dep": "dep" }
                },
                "dep": {
                    "url": "file:///app/dep/",
                    "dependencies": {}
                }
            }
        });
        let config = ModuleLoaderConfig {
            allow_external: true,
            policy_chain: None,
            virtual_modules: None,
            virtual_commonjs_modules: None,
            virtual_files: Some(Arc::new(HashSet::from([virtual_package_map(&package_map)]))),
        };

        let resolved = resolve_package_map(
            &config,
            &HashMap::new(),
            "dep",
            "file:///outside/main.mjs",
            &["import", "node", "default"],
        )
        .expect("package map should handle bare imports")
        .unwrap();
        let query = resolved.query_pairs().collect::<HashMap<_, _>>();

        assert_eq!(query.get("code").unwrap(), "ERR_PACKAGE_MAP_EXTERNAL_FILE");
    }

    #[test]
    fn package_error_modules_link_default_imports_before_throwing() {
        let source = node_error_module_source("ERR_PACKAGE_MAP_KEY_NOT_FOUND", "missing");

        assert!(source.starts_with("export default undefined;"));
        assert!(source.contains("throw error;"));
    }

}
