//! Single-file server configuration (`--config` / `MCP_V8_CONFIG`).
//!
//! One TOML or JSON file configures everything the CLI can. Scalar keys mirror
//! flag names (`http_port = 8080` ≡ `--http-port 8080`; dashes and underscores
//! are interchangeable), and four structured sections inline what is otherwise
//! a separate JSON file or inline-JSON flag:
//!
//! - `wasm`          → `--wasm-config`         (table: name → path string or `{path, max_memory_bytes, description}`)
//! - `mcp_servers`   → `--mcp-config`          (array of server objects)
//! - `fetch_headers` → `--fetch-header-config` (array of header-rule objects)
//! - `policies`      → `--policies-json`       (policies object)
//!
//! Precedence: explicit CLI flag > `MCP_V8_*` env var > config file > built-in
//! default. There is no per-flag code here to keep in sync: config values are
//! installed as each clap arg's *default value*, so clap's own resolution
//! order (command line, then env, then default) yields exactly that
//! precedence, and the key vocabulary is derived from the live
//! [`clap::Command`] — a newly added flag is configurable from the file
//! automatically, and a typo'd key fails startup listing the accepted keys.

use std::collections::BTreeMap;

use anyhow::{Context, Result, anyhow, bail};
use serde_json::Value;

/// Structured config-file sections. Each replaces a flag that accepts inline
/// JSON (or a path to a JSON file): the section value is re-serialized to JSON
/// and installed as that flag's value.
///
/// Public (with [`REJECTED_KEYS`] and [`accepted_keys`]) so that
/// `generate-config-markdown` documents exactly the tables the loader runs on.
pub struct Section {
    /// Key in the config file.
    pub key: &'static str,
    /// Arg id of the flag the section feeds (also the key of the scalar
    /// path-form twin, which therefore cannot be set in the same file).
    pub target_arg: &'static str,
    /// Expected JSON shape of the section value.
    pub shape: Shape,
}

#[derive(Clone, Copy, PartialEq)]
pub enum Shape {
    Object,
    Array,
}

impl Shape {
    pub fn describe(self) -> &'static str {
        match self {
            Shape::Object => "a table/object",
            Shape::Array => "an array",
        }
    }
}

pub const SECTIONS: &[Section] = &[
    Section { key: "wasm", target_arg: "wasm_config", shape: Shape::Object },
    Section { key: "mcp_servers", target_arg: "mcp_config", shape: Shape::Array },
    Section { key: "fetch_headers", target_arg: "fetch_header_config", shape: Shape::Array },
    Section { key: "policies", target_arg: "policies_json", shape: Shape::Object },
    Section { key: "sandbox", target_arg: "sandbox_manifest", shape: Shape::Object },
];

/// Flags that exist on the CLI but make no sense in (or are unsupported from)
/// a config file, each with the hint reported to the user.
pub const REJECTED_KEYS: &[(&str, &str)] = &[
    ("config", "a config file cannot chain-load another config file"),
    ("print_openapi", "run `--print-openapi` on the command line instead"),
    ("wasm_modules", "use the `wasm` section (or a `wasm_config` path) instead"),
    ("wasm_stub_descriptions", "set `description` on entries in the `wasm` section instead"),
];


/// Resolve the config file path the same way clap would resolve the flag:
/// `--config <path>` / `--config=<path>` from the command line, else the
/// `MCP_V8_CONFIG` environment variable.
fn config_path_from<I: IntoIterator<Item = String>>(argv: I, env_value: Option<String>) -> Option<String> {
    let mut argv = argv.into_iter();
    while let Some(token) = argv.next() {
        if token == "--config" {
            // A missing value is left for clap to report.
            return argv.next();
        }
        if let Some(path) = token.strip_prefix("--config=") {
            return Some(path.to_string());
        }
    }
    env_value.filter(|value| !value.is_empty())
}

/// Read and parse the config file into a JSON object. The format is chosen by
/// extension: `.toml` or `.json`.
fn load_document(path: &str) -> Result<serde_json::Map<String, Value>> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read config file '{path}'"))?;
    let extension = std::path::Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase);
    let doc: Value = match extension.as_deref() {
        Some("json") => serde_json::from_str(&text).with_context(|| format!("invalid JSON in '{path}'"))?,
        Some("toml") => toml::from_str(&text).with_context(|| format!("invalid TOML in '{path}'"))?,
        _ => bail!("unsupported config file extension for '{path}': expected .toml or .json"),
    };
    match doc {
        Value::Object(map) => Ok(map),
        other => bail!("config file '{path}' must contain a top-level table/object, got {}", json_type(&other)),
    }
}

fn json_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "a table/object",
    }
}

/// Map the config document onto clap arg ids and the string values to install
/// as their defaults. Pure (no process state) so it is directly testable.
fn compute_overrides(
    doc: &serde_json::Map<String, Value>,
    command: &clap::Command,
) -> Result<Vec<(String, Vec<String>)>> {
    // Key normalization: dashes and underscores are interchangeable.
    let normalized: Vec<(String, &String, &Value)> =
        doc.iter().map(|(key, value)| (key.replace('-', "_"), key, value)).collect();

    let args: BTreeMap<&str, &clap::Arg> = command
        .get_arguments()
        .map(|arg| (arg.get_id().as_str(), arg))
        .collect();

    // Clap only enforces `conflicts_with` for values that are *present* (CLI
    // or env), not for injected defaults — so conflicts between config keys
    // are checked here, against the declarations on the live command. A new
    // `conflicts_with` on any flag is picked up automatically.
    for (key, raw_key, _) in &normalized {
        let Some(arg) = args.get(key.as_str()) else { continue };
        for conflict in command.get_arg_conflicts_with(arg) {
            let conflict_id = conflict.get_id().as_str();
            if normalized.iter().any(|(other, ..)| other == conflict_id) {
                bail!(
                    "keys '{raw_key}' and '{conflict_id}' cannot both be set (same conflict as the flags)"
                );
            }
        }
    }

    let mut overrides: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut unknown: Vec<String> = Vec::new();

    for (key, raw_key, value) in &normalized {
        if let Some((_, hint)) = REJECTED_KEYS.iter().find(|(rejected, _)| rejected == key) {
            bail!("key '{raw_key}' is not allowed in a config file: {hint}");
        }

        let (arg_id, values) = if let Some(section) = SECTIONS.iter().find(|section| section.key == key) {
            if normalized.iter().any(|(other, ..)| other == section.target_arg) {
                bail!(
                    "keys '{}' and '{}' cannot both be set: use the structured '{}' section or the '{}' path, not both",
                    section.key, section.target_arg, section.key, section.target_arg
                );
            }
            let shape_ok = match section.shape {
                Shape::Object => value.is_object(),
                Shape::Array => value.is_array(),
            };
            if !shape_ok {
                bail!("section '{raw_key}' must be {}, got {}", section.shape.describe(), json_type(value));
            }
            let json = serde_json::to_string(value).expect("re-serializing parsed config value cannot fail");
            (section.target_arg.to_string(), vec![json])
        } else {
            let Some(arg) = args.get(key.as_str()).filter(|arg| arg.get_long().is_some()) else {
                unknown.push((*raw_key).clone());
                continue;
            };
            (arg.get_id().to_string(), stringify_values(arg, raw_key, value)?)
        };

        if overrides.insert(arg_id.clone(), values).is_some() {
            bail!("key '{raw_key}' is set more than once (dashes and underscores are equivalent)");
        }
    }

    if !unknown.is_empty() {
        bail!(
            "unknown key{}: {}. Accepted keys: {}",
            if unknown.len() == 1 { "" } else { "s" },
            unknown.join(", "),
            accepted_keys(command).join(", ")
        );
    }

    Ok(overrides.into_iter().collect())
}

/// Convert one config value into the string(s) clap will parse for `arg`.
fn stringify_values(arg: &clap::Arg, key: &str, value: &Value) -> Result<Vec<String>> {
    match value {
        Value::Array(items) => {
            if !matches!(arg.get_action(), clap::ArgAction::Append) {
                bail!("key '{key}' does not accept a list");
            }
            items
                .iter()
                .map(|item| {
                    scalar_to_string(item)
                        .ok_or_else(|| anyhow!("entries of '{key}' must be strings, numbers, or booleans"))
                })
                .collect()
        }
        other => Ok(vec![scalar_to_string(other).ok_or_else(|| {
            anyhow!("key '{key}' must be a string, number, or boolean, got {}", json_type(other))
        })?]),
    }
}

fn scalar_to_string(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// Every key a config file may set, for the unknown-key error and the
/// generated reference page: the structured sections plus each flag-backed
/// arg id, minus the rejected keys and the grammar-string flags the sections
/// replace.
pub fn accepted_keys(command: &clap::Command) -> Vec<String> {
    let mut keys: Vec<String> = SECTIONS.iter().map(|section| section.key.to_string()).collect();
    for arg in command.get_arguments() {
        let id = arg.get_id().as_str();
        if arg.get_long().is_none()
            || REJECTED_KEYS.iter().any(|(rejected, _)| *rejected == id)
            || SECTIONS.iter().any(|section| section.key == id)
        {
            continue;
        }
        keys.push(id.to_string());
    }
    keys.sort();
    keys
}

/// Install the computed values as the args' default values. Clap resolves
/// command line, then env, then defaults — which is exactly the documented
/// config-file precedence.
fn apply_overrides(mut command: clap::Command, overrides: Vec<(String, Vec<String>)>) -> clap::Command {
    for (arg_id, values) in overrides {
        command = command.mut_arg(arg_id.as_str(), move |arg| arg.default_values(values.clone()));
    }
    command
}

/// Entry point used by [`crate::cli::parse`]: if `--config`/`MCP_V8_CONFIG`
/// names a file, fold its contents into `command` as per-arg defaults.
/// Config-file errors exit through clap so they render like other CLI errors.
pub fn apply_config_file(mut command: clap::Command) -> clap::Command {
    let Some(path) = config_path_from(std::env::args().skip(1), std::env::var("MCP_V8_CONFIG").ok()) else {
        return command;
    };
    let overrides = load_document(&path).and_then(|doc| compute_overrides(&doc, &command));
    match overrides {
        Ok(overrides) => apply_overrides(command, overrides),
        Err(err) => command
            .error(clap::error::ErrorKind::ValueValidation, format!("--config {path}: {err:#}"))
            .exit(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{Cli, StoreKind, build_command};
    use clap::FromArgMatches;

    fn parse_toml(toml_text: &str) -> serde_json::Map<String, Value> {
        match toml::from_str::<Value>(toml_text).expect("test TOML must parse") {
            Value::Object(map) => map,
            _ => unreachable!("TOML documents are tables"),
        }
    }

    /// Parse `argv` (without the binary name) with the given config document
    /// folded in, mirroring the production path minus process env/argv.
    fn parse_with_config(toml_text: &str, argv: &[&str]) -> Cli {
        let command = build_command();
        let overrides = compute_overrides(&parse_toml(toml_text), &command).expect("config must be valid");
        let command = apply_overrides(command, overrides);
        let mut matches = command
            .try_get_matches_from(std::iter::once("server").chain(argv.iter().copied()))
            .expect("argv must parse");
        Cli::from_arg_matches_mut(&mut matches).expect("Cli must build from matches")
    }

    fn config_error(toml_text: &str) -> String {
        let command = build_command();
        format!("{:#}", compute_overrides(&parse_toml(toml_text), &command).expect_err("config must be rejected"))
    }

    #[test]
    fn scalar_keys_map_to_flags() {
        let cli = parse_with_config(
            r#"
            http-port = 8080          # kebab-case accepted
            heap_store = "dir"
            heap_dir = "/data/heaps"
            heap_memory_max = 64
            allow_external_modules = true
            node_globals = true
            instructions = "Run JS for me"
            "#,
            &[],
        );
        assert_eq!(cli.http_port, Some(8080));
        assert_eq!(cli.heap_store, StoreKind::Dir);
        assert_eq!(cli.heap_dir.as_deref(), Some("/data/heaps"));
        assert_eq!(cli.heap_memory_max, 64);
        assert!(cli.allow_external_modules);
        assert!(cli.node_globals);
        assert_eq!(cli.instructions.as_deref(), Some("Run JS for me"));
    }

    #[test]
    fn cli_flags_beat_config_values() {
        let cli = parse_with_config("http_port = 8080\nheap_memory_max = 64", &["--http-port", "9090"]);
        assert_eq!(cli.http_port, Some(9090), "explicit CLI flag must win");
        assert_eq!(cli.heap_memory_max, 64, "untouched keys still come from the config");
    }

    #[test]
    fn env_vars_beat_config_values() {
        // Env resolution is clap's; drive it through a private env var name to
        // stay independent of the real process environment. Clap captures the
        // variable's value when `.env()` is called, so it must be set first.
        // SAFETY: test-only variable name.
        unsafe { std::env::set_var("TEST_ONLY_MCP_V8_EXECUTION_TIMEOUT", "200") };
        let command = build_command();
        let overrides = compute_overrides(&parse_toml("execution_timeout = 100"), &command).unwrap();
        let command = apply_overrides(command, overrides)
            .mut_arg("execution_timeout", |arg| arg.env("TEST_ONLY_MCP_V8_EXECUTION_TIMEOUT"));
        let mut matches = command.try_get_matches_from(["server"]).unwrap();
        let cli = Cli::from_arg_matches_mut(&mut matches).unwrap();
        unsafe { std::env::remove_var("TEST_ONLY_MCP_V8_EXECUTION_TIMEOUT") };
        assert_eq!(cli.execution_timeout, 200, "env var must beat the config file");
    }

    #[test]
    fn node_globals_env_beats_config() {
        unsafe { std::env::set_var("TEST_ONLY_MCP_V8_NODE_GLOBALS", "true") };
        let command = build_command();
        let overrides = compute_overrides(&parse_toml("node_globals = false"), &command).unwrap();
        let command = apply_overrides(command, overrides)
            .mut_arg("node_globals", |arg| arg.env("TEST_ONLY_MCP_V8_NODE_GLOBALS"));
        let mut matches = command.try_get_matches_from(["server"]).unwrap();
        let cli = Cli::from_arg_matches_mut(&mut matches).unwrap();
        unsafe { std::env::remove_var("TEST_ONLY_MCP_V8_NODE_GLOBALS") };
        assert!(cli.node_globals, "env var must beat the config file");
    }

    #[test]
    fn repeatable_flags_accept_arrays() {
        let cli = parse_with_config(r#"peers = ["node2@10.0.0.2:4000", "10.0.0.3:4000"]"#, &[]);
        assert_eq!(cli.peers, ["node2@10.0.0.2:4000", "10.0.0.3:4000"]);
    }

    #[test]
    fn structured_sections_feed_the_inline_json_flags() {
        let cli = parse_with_config(
            r#"
            [wasm.math]
            path = "/modules/math.wasm"
            max_memory_bytes = 16777216
            description = "adds numbers"

            [[mcp_servers]]
            name = "weather"
            transport = "stdio"
            command = "python"
            args = ["server.py"]

            [[fetch_headers]]
            host = "api.github.com"
            headers = { Authorization = "Bearer x" }

            [policies.fetch]
            mode = "all"
            policies = [{ url = "file:///policies/fetch.rego" }]
            "#,
            &[],
        );

        let wasm: Value = serde_json::from_str(cli.wasm_config.as_deref().unwrap()).unwrap();
        assert_eq!(wasm["math"]["path"], "/modules/math.wasm");
        assert_eq!(wasm["math"]["max_memory_bytes"], 16777216);

        let mcp: Value = serde_json::from_str(cli.mcp_config.as_deref().unwrap()).unwrap();
        assert_eq!(mcp[0]["name"], "weather");
        assert_eq!(mcp[0]["transport"], "stdio");

        let headers: Value = serde_json::from_str(cli.fetch_header_config.as_deref().unwrap()).unwrap();
        assert_eq!(headers[0]["host"], "api.github.com");

        let policies: Value = serde_json::from_str(cli.policies_json.as_deref().unwrap()).unwrap();
        assert_eq!(policies["fetch"]["mode"], "all");
    }

    #[test]
    fn toml_mcp_server_oauth_browser_config_parses() {
        let cli = parse_with_config(
            r#"
            [[mcp_servers]]
            name = "protected-api"
            transport = "http"
            url = "https://api.example.com/mcp"

            [mcp_servers.auth]
            type = "oauth_browser"
            scope = ["read"]
            client_id = "client-id"
            "#,
            &[],
        );

        let mcp: Value = serde_json::from_str(cli.mcp_config.as_deref().unwrap()).unwrap();
        assert_eq!(mcp[0]["name"], "protected-api");
        assert_eq!(mcp[0]["auth"]["type"], "oauth_browser");
        assert_eq!(mcp[0]["auth"]["scope"], serde_json::json!(["read"]));
        assert_eq!(mcp[0]["auth"]["client_id"], "client-id");
    }

    #[test]
    fn unknown_keys_are_rejected_with_the_accepted_list() {
        let err = config_error("htpp_port = 8080");
        assert!(err.contains("unknown key: htpp_port"), "got: {err}");
        assert!(err.contains("http_port"), "accepted-key list must be shown: {err}");
        assert!(err.contains("policies"), "sections must be listed too: {err}");
    }

    #[test]
    fn cli_only_keys_are_rejected() {
        for (toml_text, needle) in [
            ("config = \"other.toml\"", "chain-load"),
            ("print_openapi = true", "command line"),
            ("wasm_modules = [\"m=/m.wasm\"]", "`wasm` section"),
            ("wasm_stub_descriptions = [\"m=text\"]", "`wasm` section"),
        ] {
            let err = config_error(toml_text);
            assert!(err.contains(needle), "for {toml_text}: {err}");
        }
    }

    #[test]
    fn section_and_path_twin_cannot_both_be_set() {
        let err = config_error("wasm_config = \"/etc/wasm.json\"\n[wasm.math]\npath = \"/m.wasm\"");
        assert!(err.contains("'wasm' and 'wasm_config'"), "got: {err}");
    }

    #[test]
    fn clap_conflicts_are_enforced_between_config_keys() {
        let err = config_error("http_port = 8080\nsse_port = 8081");
        assert!(err.contains("'http_port' and 'sse_port'"), "got: {err}");
    }

    #[test]
    fn list_for_single_value_flag_is_rejected() {
        let err = config_error("http_port = [8080, 8081]");
        assert!(err.contains("does not accept a list"), "got: {err}");
    }

    #[test]
    fn wrong_section_shape_is_rejected() {
        let err = config_error("mcp_servers = \"weather=stdio:python\"");
        assert!(err.contains("must be an array"), "got: {err}");
    }

    #[test]
    fn duplicate_keys_across_spellings_are_rejected() {
        let err = config_error("http_port = 8080\n\"http-port\" = 9090");
        assert!(err.contains("more than once"), "got: {err}");
    }

    // ── Drift guards ─────────────────────────────────────────────────────
    // Everything else in this module is derived from the live clap command
    // (key vocabulary, value parsing, conflicts), but REJECTED_KEYS and
    // SECTIONS name arg ids as strings. This pins them to the real args so a
    // flag rename breaks the build's tests instead of a user's startup.

    #[test]
    fn rejected_keys_and_sections_name_real_args() {
        use std::collections::BTreeSet;

        let command = build_command();
        let ids: BTreeSet<&str> = command.get_arguments().map(|arg| arg.get_id().as_str()).collect();

        for (key, _) in REJECTED_KEYS {
            assert!(
                ids.contains(key),
                "REJECTED_KEYS entry '{key}' is not an arg id on the command; update it alongside the flag rename"
            );
        }
        for section in SECTIONS {
            let target = command
                .get_arguments()
                .find(|arg| arg.get_id().as_str() == section.target_arg)
                .unwrap_or_else(|| {
                    panic!(
                        "section '{}' targets arg '{}', which is not on the command; update it alongside the flag rename",
                        section.key, section.target_arg
                    )
                });
            // The section value is serialized to JSON and installed as the
            // target's single value, so the target must take one.
            assert!(
                target.get_action().takes_values(),
                "section '{}' target '{}' must take a value",
                section.key,
                section.target_arg
            );
        }
    }

    #[test]
    fn config_path_is_found_in_argv_or_env() {
        let argv = |tokens: &[&str]| tokens.iter().map(|t| t.to_string()).collect::<Vec<_>>();
        assert_eq!(
            config_path_from(argv(&["--stateless", "--config", "a.toml"]), None).as_deref(),
            Some("a.toml")
        );
        assert_eq!(config_path_from(argv(&["--config=b.toml"]), None).as_deref(), Some("b.toml"));
        assert_eq!(
            config_path_from(argv(&[]), Some("c.toml".to_string())).as_deref(),
            Some("c.toml"),
            "MCP_V8_CONFIG is the fallback"
        );
        assert_eq!(
            config_path_from(argv(&["--config", "a.toml"]), Some("c.toml".to_string())).as_deref(),
            Some("a.toml"),
            "the flag beats the env var"
        );
        assert_eq!(config_path_from(argv(&["--config"]), None), None, "missing value is left to clap");
    }
}
