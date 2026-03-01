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

/// Module loader that resolves `npm:`, `jsr:`, and URL imports by fetching
/// them from the network. NPM and JSR specifiers are rewritten to esm.sh
/// URLs so that packages are served as standard ES modules.
pub struct NetworkModuleLoader {
    client: reqwest::Client,
}

impl NetworkModuleLoader {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
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
            let url = format!("https://esm.sh/{}", rest);
            return ModuleSpecifier::parse(&url)
                .map_err(|e| JsErrorBox::generic(format!("Bad npm specifier '{}': {}", specifier, e)));
        }

        // jsr:@luca/cases@1.0.0 → https://esm.sh/jsr/@luca/cases@1.0.0
        if let Some(rest) = specifier.strip_prefix("jsr:") {
            let url = format!("https://esm.sh/jsr/{}", rest);
            return ModuleSpecifier::parse(&url)
                .map_err(|e| JsErrorBox::generic(format!("Bad jsr specifier '{}': {}", specifier, e)));
        }

        // Absolute URLs pass through directly.
        if specifier.starts_with("https://") || specifier.starts_with("http://") {
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
        let fut = async move {
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
