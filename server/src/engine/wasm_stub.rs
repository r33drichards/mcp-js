//! WASM module stubs for the MCPJS MCP surface.
//!
//! Pre-loaded WASM modules (`--wasm-module`, `--wasm-config`) are injected
//! into the JavaScript runtime as globals (`__wasm_<name>`, and for
//! import-free modules an instantiated `<name>` exports object). On their own
//! they are invisible to a downstream MCP client: the agent has no way to know
//! a module is available without reading server configuration.
//!
//! Mirroring the upstream MCP server tool stubs (see `mcp_client.rs`), this
//! module lets MCPJS advertise each loaded WASM module as a stub tool on its
//! own MCP surface. The stub:
//!
//!   * appears in `tools/list` / tool search with a name like
//!     `runjs__wasm__<name>`,
//!   * carries a description explaining the module's globals, exports, and
//!     whether it needs an imports object, and
//!   * when called, returns instructional text telling the agent to use the
//!     module from JavaScript via `run_js` rather than executing directly.
//!
//! The stub is intentionally not an executable proxy — WASM exports have no
//! MCP-level schema, so the agent drives them through `run_js`.

use rmcp::model::{CallToolResult, Content, Tool};
use serde_json::json;
use std::sync::Arc;

use super::WasmModule;


/// Default prefix for WASM stub tool names. Shared with the MCP tool stubs:
/// `runjs__` signals to a calling agent that the capability executes through
/// the JavaScript runtime rather than dispatching directly.
pub const DEFAULT_WASM_STUB_PREFIX: &str = "runjs__";

/// Infix that distinguishes WASM module stubs from upstream MCP server stubs.
/// A full stub name is `<prefix>wasm__<module>`, e.g. `runjs__wasm__sqlite`.
const WASM_INFIX: &str = "wasm__";

/// Configuration for the auto-generated WASM module stubs MCPJS exposes to its
/// own clients. The default prefix `runjs__` makes it obvious that these tools
/// execute indirectly through the JavaScript runtime (`run_js`).

pub struct WasmStubConfig {
    pub prefix: String,
    pub enabled: bool,
}

impl Default for WasmStubConfig {
    fn default() -> Self {
        Self {
            prefix: DEFAULT_WASM_STUB_PREFIX.to_string(),
            enabled: true,
        }
    }
}


/// Build the stub tool name for a WASM module under the given `prefix`.
pub fn wasm_stub_tool_name(prefix: &str, module: &str) -> String {
    format!("{}{}{}", prefix, WASM_INFIX, module)
}

/// Inverse of `wasm_stub_tool_name`. Returns the module name, or `None` if
/// `name` does not start with `prefix` followed by the `wasm__` infix, or if
/// the remaining module segment is empty. An empty `prefix` is treated as "no
/// stub recognition" and always returns `None`.
pub fn parse_wasm_stub_tool_name(prefix: &str, name: &str) -> Option<String> {
    if prefix.is_empty() {
        return None;
    }
    let rest = name.strip_prefix(prefix)?;
    let module = rest.strip_prefix(WASM_INFIX)?;
    if module.is_empty() {
        return None;
    }
    Some(module.to_string())
}


/// Lightweight summary of a WASM module's surface, parsed from its bytes.
struct ModuleSurface {
    has_imports: bool,
    exports: Vec<String>,
}

/// Parse export names and detect imports without instantiating the module.
/// Parse failures degrade gracefully (treated as "no imports, no exports")
/// because the bytes were already validated when the module was loaded.
fn module_surface(bytes: &[u8]) -> ModuleSurface {
    use wasmparser::{Parser, Payload};

    let mut has_imports = false;
    let mut exports = Vec::new();
    for payload in Parser::new(0).parse_all(bytes) {
        match payload {
            Ok(Payload::ImportSection(reader)) => {
                if reader.count() > 0 {
                    has_imports = true;
                }
            }
            Ok(Payload::ExportSection(reader)) => {
                for export in reader.into_iter().flatten() {
                    exports.push(export.name.to_string());
                }
            }
            _ => {}
        }
    }
    ModuleSurface {
        has_imports,
        exports,
    }
}


/// Build a stub `Tool` describing a loaded WASM module. Calling the tool only
/// returns usage instructions; it does not execute the module.
pub fn make_wasm_stub_tool(prefix: &str, module: &WasmModule) -> Tool {
    let stub_name = wasm_stub_tool_name(prefix, &module.name);
    let surface = module_surface(&module.bytes);
    let description = stub_description(&module.name, &surface, module.description.as_deref());

                let input_schema = json!({
        "type": "object",
        "properties": {},
    });
    let input_schema = Arc::new(
        input_schema
            .as_object()
            .cloned()
            .expect("object literal is an object"),
    );

    Tool {
        name: stub_name.into(),
        description: Some(description.into()),
        input_schema,
                        annotations: None,
    }
}

/// Human-readable description placed on the stub tool, explaining how to use
/// the module from JavaScript. An operator-supplied `custom` description (from
/// `--wasm-stub-description` / `--wasm-config`) is shown prominently right
/// after the stub header, before the auto-generated usage hint.
fn stub_description(name: &str, surface: &ModuleSurface, custom: Option<&str>) -> String {
    let module_global = format!("__wasm_{}", name);
    let mut desc = format!(
        "[stub for pre-loaded WASM module {name} — use it from JavaScript via \
         the run_js tool. Calling this tool directly only returns instructions; \
         it does not execute the module.]",
        name = name,
    );

    if let Some(custom) = custom {
        let custom = custom.trim();
        if !custom.is_empty() {
            desc.push_str(&format!("\n\n{}", custom));
        }
    }

    desc.push_str(&format!(
        "\n\nThe compiled WebAssembly.Module is available as the global \
         `{module_global}`.",
        module_global = module_global,
    ));

    if surface.has_imports {
        desc.push_str(&format!(
            " This module declares imports, so instantiate it with an imports \
             object in JS:\n  const instance = new WebAssembly.Instance({module_global}, imports);\n  \
             instance.exports.<fn>(...);",
            module_global = module_global,
        ));
    } else {
        desc.push_str(&format!(
            " This module has no imports, so its exports are also auto-instantiated \
             and bound to the global `{name}`:\n  {name}.<fn>(...);\nor instantiate \
             the module manually:\n  const instance = new WebAssembly.Instance({module_global});\n  \
             instance.exports.<fn>(...);",
            name = name,
            module_global = module_global,
        ));
    }

    if !surface.exports.is_empty() {
        desc.push_str(&format!("\n\nExports: {}.", surface.exports.join(", ")));
    }

    desc
}

/// Render the instructional text returned when a client calls a WASM stub.
pub fn wasm_stub_instructions(
    module: &WasmModule,
    arguments: Option<&serde_json::Map<String, serde_json::Value>>,
) -> String {
    let surface = module_surface(&module.bytes);
    let module_global = format!("__wasm_{}", module.name);

    let example = if surface.has_imports {
        format!(
            "const instance = new WebAssembly.Instance({module_global}, {{ /* imports */ }});\n\
             const result = instance.exports.someFunction(/* args */);\n\
             console.log(result);",
            module_global = module_global,
        )
    } else {
        format!(
            "// Exports are auto-bound to the global `{name}`:\n\
             const result = {name}.someFunction(/* args */);\n\
             console.log(result);",
            name = module.name,
        )
    };

    let mut text = format!(
        "This tool is a stub for the WASM module {name:?}. Execute it from \
         JavaScript via the `run_js` tool, e.g.:\n\n{example}\n",
        name = module.name,
        example = example,
    );

    if !surface.exports.is_empty() {
        text.push_str(&format!(
            "\nAvailable exports: {}.\n",
            surface.exports.join(", ")
        ));
    }

    if let Some(args) = arguments {
        if !args.is_empty() {
            let pretty = serde_json::to_string_pretty(&serde_json::Value::Object(args.clone()))
                .unwrap_or_else(|_| "{}".into());
            text.push_str(&format!(
                "\nArguments you passed to this stub were:\n{}\nPass them to the \
                 relevant export(s) inside run_js.\n",
                pretty
            ));
        }
    }

    text
}


/// Generate stub `Tool` definitions for every loaded WASM module. Returns an
/// empty vec when stub exposure is disabled in the config.
pub fn stub_tools(modules: &[WasmModule], config: &WasmStubConfig) -> Vec<Tool> {
    if !config.enabled {
        return Vec::new();
    }
    modules
        .iter()
        .map(|m| make_wasm_stub_tool(&config.prefix, m))
        .collect()
}

/// If `name` is a stub for a known WASM module, build the instructional
/// `CallToolResult`. Returns `None` if stubs are disabled or if `name` does
/// not match any known module — callers should fall through to their normal
/// tool dispatcher in that case.
pub fn stub_call_response(
    modules: &[WasmModule],
    config: &WasmStubConfig,
    name: &str,
    arguments: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Option<CallToolResult> {
    if !config.enabled {
        return None;
    }
    let module_name = parse_wasm_stub_tool_name(&config.prefix, name)?;
    let module = modules.iter().find(|m| m.name == module_name)?;
    Some(CallToolResult::success(vec![Content::text(
        wasm_stub_instructions(module, arguments),
    )]))
}



mod tests {
    use super::*;
    use serde_json::json;

    /// Import-free WASM module exporting `add(i32, i32) -> i32`. Hand-assembled
    /// bytes (same style as `server/tests/wasm.rs`) so tests need no extra deps.
    fn module_with_export() -> Vec<u8> {
        vec![
            0x00, 0x61, 0x73, 0x6d,             0x01, 0x00, 0x00, 0x00,             0x01, 0x07, 0x01, 0x60, 0x02, 0x7f, 0x7f, 0x01, 0x7f,             0x03, 0x02, 0x01, 0x00,             0x07, 0x07, 0x01, 0x03, 0x61, 0x64, 0x64, 0x00, 0x00,             0x0a, 0x09, 0x01, 0x07, 0x00, 0x20, 0x00, 0x20, 0x01, 0x6a, 0x0b,         ]
    }

    /// Module importing `env.log(i32)` and exporting `run(i32)` — requires an
    /// imports object, so it exercises the `has_imports` branch.
    fn module_with_import() -> Vec<u8> {
        vec![
            0x00, 0x61, 0x73, 0x6d,             0x01, 0x00, 0x00, 0x00,             0x01, 0x05, 0x01, 0x60, 0x01, 0x7f, 0x00,                         0x02, 0x0b, 0x01, 0x03, 0x65, 0x6e, 0x76, 0x03, 0x6c, 0x6f, 0x67, 0x00, 0x00,
            0x03, 0x02, 0x01, 0x00,                         0x07, 0x07, 0x01, 0x03, 0x72, 0x75, 0x6e, 0x00, 0x01,
                        0x0a, 0x08, 0x01, 0x06, 0x00, 0x20, 0x00, 0x10, 0x00, 0x0b,
        ]
    }

    fn module(name: &str, bytes: Vec<u8>) -> WasmModule {
        WasmModule {
            name: name.to_string(),
            bytes,
            max_memory_bytes: None,
            description: None,
        }
    }

    
    fn default_prefix_is_runjs() {
        assert_eq!(WasmStubConfig::default().prefix, "runjs__");
        assert_eq!(DEFAULT_WASM_STUB_PREFIX, "runjs__");
        assert!(WasmStubConfig::default().enabled);
    }

    
    fn stub_name_round_trips() {
        let n = wasm_stub_tool_name("runjs__", "sqlite");
        assert_eq!(n, "runjs__wasm__sqlite");
        assert_eq!(
            parse_wasm_stub_tool_name("runjs__", &n),
            Some("sqlite".to_string())
        );
    }

    
    fn stub_name_round_trips_with_custom_prefix() {
        let n = wasm_stub_tool_name("rj_", "math");
        assert_eq!(n, "rj_wasm__math");
        assert_eq!(parse_wasm_stub_tool_name("rj_", &n), Some("math".to_string()));
                assert_eq!(parse_wasm_stub_tool_name("runjs__", &n), None);
    }

    
    fn parse_rejects_non_stub_names() {
                assert_eq!(parse_wasm_stub_tool_name("runjs__", "run_js"), None);
                assert_eq!(
            parse_wasm_stub_tool_name("runjs__", "runjs__github__create_issue"),
            None
        );
                assert_eq!(parse_wasm_stub_tool_name("runjs__", "runjs__wasm__"), None);
                assert_eq!(parse_wasm_stub_tool_name("", "wasm__math"), None);
    }

    
    fn module_surface_detects_exports_and_no_imports() {
        let surface = module_surface(&module_with_export());
        assert!(!surface.has_imports);
        assert_eq!(surface.exports, vec!["add".to_string()]);
    }

    
    fn module_surface_detects_imports() {
        let surface = module_surface(&module_with_import());
        assert!(surface.has_imports);
        assert_eq!(surface.exports, vec!["run".to_string()]);
    }

    
    fn make_stub_tool_for_import_free_module() {
        let m = module("math", module_with_export());
        let stub = make_wasm_stub_tool("runjs__", &m);
        assert_eq!(stub.name, "runjs__wasm__math");
        let desc = stub.description.unwrap();
        assert!(desc.contains("__wasm_math"), "desc: {}", desc);
        assert!(desc.contains("run_js"), "desc: {}", desc);
                assert!(desc.contains("auto-instantiated"), "desc: {}", desc);
        assert!(desc.contains("add"), "desc should list exports: {}", desc);
                let schema = serde_json::to_value(stub.input_schema.as_ref()).unwrap();
        assert_eq!(schema["type"], json!("object"));
    }

    
    fn make_stub_tool_includes_custom_description() {
        let mut m = module("math", module_with_export());
        m.description = Some("Fast fixed-point arithmetic helpers.".to_string());
        let stub = make_wasm_stub_tool("runjs__", &m);
        let desc = stub.description.unwrap();
                assert!(
            desc.contains("Fast fixed-point arithmetic helpers."),
            "desc: {}",
            desc
        );
                assert!(desc.contains("__wasm_math"), "desc: {}", desc);
        assert!(desc.contains("run_js"), "desc: {}", desc);
        assert!(desc.contains("add"), "desc: {}", desc);
    }

    
    fn make_stub_tool_for_importing_module() {
        let m = module("logger", module_with_import());
        let stub = make_wasm_stub_tool("runjs__", &m);
        let desc = stub.description.unwrap();
        assert!(desc.contains("imports object"), "desc: {}", desc);
        assert!(desc.contains("WebAssembly.Instance"), "desc: {}", desc);
    }

    
    fn stub_tools_lists_every_module() {
        let modules = vec![
            module("math", module_with_export()),
            module("logger", module_with_import()),
        ];
        let mut names: Vec<String> = stub_tools(&modules, &WasmStubConfig::default())
            .into_iter()
            .map(|t| t.name.to_string())
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                "runjs__wasm__logger".to_string(),
                "runjs__wasm__math".to_string(),
            ]
        );
    }

    
    fn stub_tools_empty_when_disabled() {
        let modules = vec![module("math", module_with_export())];
        let config = WasmStubConfig {
            prefix: "runjs__".to_string(),
            enabled: false,
        };
        assert!(stub_tools(&modules, &config).is_empty());
        assert!(stub_call_response(&modules, &config, "runjs__wasm__math", None).is_none());
    }

    
    fn stub_call_response_matches_known_module() {
        let modules = vec![module("math", module_with_export())];
        let config = WasmStubConfig::default();
        let mut args = serde_json::Map::new();
        args.insert("x".into(), json!(1));
        let resp = stub_call_response(&modules, &config, "runjs__wasm__math", Some(&args))
            .expect("known module should match");
        assert_eq!(resp.is_error, Some(false));
        let text = serde_json::to_value(&resp.content[0]).unwrap();
        let text = text["text"].as_str().unwrap_or_default();
        assert!(text.contains("run_js"), "text: {}", text);
        assert!(text.contains("math"), "text: {}", text);
        assert!(text.contains("add"), "text should list exports: {}", text);
        assert!(text.contains("\"x\""), "text should echo args: {}", text);
    }

    
    fn stub_call_response_returns_none_for_unknowns() {
        let modules = vec![module("math", module_with_export())];
        let config = WasmStubConfig::default();
                assert!(stub_call_response(&modules, &config, "run_js", None).is_none());
                assert!(stub_call_response(&modules, &config, "runjs__wasm__other", None).is_none());
                assert!(
            stub_call_response(&modules, &config, "runjs__github__create_issue", None).is_none()
        );
    }

    
    fn stub_call_response_honours_custom_prefix() {
        let modules = vec![module("math", module_with_export())];
        let config = WasmStubConfig {
            prefix: "rj_".to_string(),
            enabled: true,
        };
        assert!(stub_call_response(&modules, &config, "rj_wasm__math", None).is_some());
                assert!(stub_call_response(&modules, &config, "runjs__wasm__math", None).is_none());
    }
}
