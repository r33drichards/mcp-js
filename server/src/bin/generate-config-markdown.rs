//! Generates the exhaustive configuration-file reference
//! (site-docs/reference/config-file.md) from the same sources the `--config`
//! loader runs on: the Clap `Cli` definition plus `config::SECTIONS` and
//! `config::REJECTED_KEYS`. The enumeration is cross-checked against
//! `config::accepted_keys` (the loader's own key vocabulary), so this page
//! cannot document a key the loader rejects or omit one it accepts.

use std::collections::{BTreeMap, BTreeSet};

use clap::ArgAction;
use server::cli::build_command;
use server::config::{REJECTED_KEYS, SECTIONS, accepted_keys};

fn slug(text: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

fn normalize(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn help_text(arg: &clap::Arg) -> String {
    arg.get_long_help()
        .or_else(|| arg.get_help())
        .map(|value| normalize(&value.to_string()))
        .unwrap_or_default()
}

fn render_default(arg: &clap::Arg, key: &str) -> Option<String> {
    // Some Clap defaults are computed at runtime from the host environment.
    // Omitting those values keeps the generated reference deterministic.
    if key == "max_concurrent_executions" {
        return None;
    }

    let defaults: Vec<String> = arg
        .get_default_values()
        .iter()
        .map(|value| value.to_string_lossy().to_string())
        .collect();

    (!defaults.is_empty()).then(|| format!("- Default: `{}`", defaults.join("`, `")))
}

/// One `### key` block for a regular (non-section) config key.
fn key_block(arg: &clap::Arg, key: &str) -> String {
    let mut block = Vec::new();
    block.push(format!("### `{key}`"));
    block.push(String::new());

    let help = help_text(arg);
    if !help.is_empty() {
        block.push(help);
        block.push(String::new());
    }

    block.push(format!("- CLI flag: `--{}`", arg.get_long().expect("configurable keys have long flags")));
    if let Some(env) = arg.get_env() {
        block.push(format!("- Environment: `{}`", env.to_string_lossy()));
    }
    if let Some(default_line) = render_default(arg, key) {
        block.push(default_line);
    }
    let possible: Vec<String> = arg
        .get_possible_values()
        .iter()
        .map(|value| format!("`{}`", value.get_name()))
        .collect();
    if !possible.is_empty() {
        block.push(format!("- Possible values: {}", possible.join(", ")));
    }
    if matches!(arg.get_action(), ArgAction::SetTrue | ArgAction::SetFalse) {
        block.push("- Type: boolean".to_string());
    }
    if matches!(arg.get_action(), ArgAction::Append) {
        block.push("- Type: array (one element per flag repetition)".to_string());
    }

    block.push(String::new());
    block.join("\n")
}

fn main() {
    let command = build_command();

    let section_keys: BTreeSet<&str> = SECTIONS.iter().map(|section| section.key).collect();
    let rejected: BTreeSet<&str> = REJECTED_KEYS.iter().map(|(key, _)| *key).collect();
    let args_by_id: BTreeMap<&str, &clap::Arg> = command
        .get_arguments()
        .map(|arg| (arg.get_id().as_str(), arg))
        .collect();

    // heading -> rendered key blocks, plus the flat key list for the
    // consistency check against the loader's own vocabulary.
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut documented: BTreeSet<String> = BTreeSet::new();

    for arg in command.get_arguments() {
        let id = arg.get_id().as_str();
        if arg.get_long().is_none() || rejected.contains(id) || section_keys.contains(id) {
            continue;
        }
        let heading = arg
            .get_help_heading()
            .map(ToString::to_string)
            .unwrap_or_else(|| "Other".to_string());
        groups.entry(heading).or_default().push(key_block(arg, id));
        documented.insert(id.to_string());
    }

    // The generated page must cover exactly the loader's key vocabulary.
    let mut expected: BTreeSet<String> = accepted_keys(&command).into_iter().collect();
    for section in SECTIONS {
        expected.remove(section.key);
    }
    assert_eq!(
        documented, expected,
        "generated config reference disagrees with config::accepted_keys — the generator's filter drifted from the loader's"
    );

    let mut lines = vec![
        "# Configuration file".to_string(),
        String::new(),
        "> Generated from the Clap `Cli` definition and the `--config` loader's".to_string(),
        "> section/rejected-key tables. Do not edit this page by hand.".to_string(),
        String::new(),
        "One TOML or JSON file, passed via `--config <PATH>` (environment:".to_string(),
        "`MCP_V8_CONFIG`), configures everything the CLI can. The format is chosen".to_string(),
        "by file extension: `.toml` or `.json`.".to_string(),
        String::new(),
        "```bash".to_string(),
        "mcp-v8 --config /etc/mcp-v8/server.toml".to_string(),
        "```".to_string(),
        String::new(),
        "## Precedence".to_string(),
        String::new(),
        "Each setting is resolved independently, highest priority first:".to_string(),
        String::new(),
        "1. Explicit command-line flag".to_string(),
        "2. `MCP_V8_*` environment variable".to_string(),
        "3. Config file".to_string(),
        "4. Built-in default".to_string(),
        String::new(),
        "## Value forms".to_string(),
        String::new(),
        "Keys are named after their CLI flag; dashes and underscores are".to_string(),
        "interchangeable (`http-port` ≡ `http_port`). Values are parsed exactly like".to_string(),
        "flag values, so anything the flag accepts, the key accepts; scalars may be".to_string(),
        "written as strings, numbers, or booleans. Keys marked as arrays take one".to_string(),
        "array element per repetition of the flag. Relative paths are resolved".to_string(),
        "against the server's working directory, not the config file's location.".to_string(),
        String::new(),
        "Every violation is a fatal startup error: unknown keys (the error lists".to_string(),
        "every accepted key), keys set twice (counting both spellings), values of".to_string(),
        "the wrong shape, and keys whose flags Clap declares as conflicting (e.g.".to_string(),
        "`http_port` and `sse_port`).".to_string(),
        String::new(),
        "## Sections".to_string(),
        String::new(),
        "- [Structured sections](#structured-sections)".to_string(),
    ];

    for heading in groups.keys() {
        lines.push(format!("- [{}](#{})", heading, slug(heading)));
    }
    lines.push("- [Keys not available in config files](#keys-not-available-in-config-files)".to_string());
    lines.push(String::new());

    lines.push("## Structured sections".to_string());
    lines.push(String::new());
    lines.push("These keys hold structured data inline, replacing what is otherwise a".to_string());
    lines.push("separate JSON file (or inline-JSON flag value). Each section is".to_string());
    lines.push("re-serialized to JSON and handed to its target flag's loader, so the".to_string());
    lines.push("schema is exactly the target flag's. A section and its target key (e.g.".to_string());
    lines.push("`wasm` and `wasm_config`) cannot both be set in the same file.".to_string());
    lines.push(String::new());
    for section in SECTIONS {
        let target = args_by_id
            .get(section.target_arg)
            .unwrap_or_else(|| panic!("section '{}' targets unknown arg '{}'", section.key, section.target_arg));
        lines.push(format!("### `{}`", section.key));
        lines.push(String::new());
        lines.push(format!(
            "Shape: {}. Feeds `--{}` (schema below).",
            section.shape.describe(),
            target.get_long().expect("section targets are long flags"),
        ));
        lines.push(String::new());
        let help = help_text(target);
        if !help.is_empty() {
            lines.push(help);
            lines.push(String::new());
        }
    }

    for (heading, blocks) in groups {
        lines.push(format!("## {heading}"));
        lines.push(String::new());
        for block in blocks {
            lines.push(block);
        }
    }

    lines.push("## Keys not available in config files".to_string());
    lines.push(String::new());
    for (key, hint) in REJECTED_KEYS {
        lines.push(format!("- `{key}` — {hint}."));
    }
    lines.push(String::new());

    lines.push("## Example".to_string());
    lines.push(String::new());
    lines.push("```toml".to_string());
    lines.push("# /etc/mcp-v8/server.toml".to_string());
    lines.push("http_port = 8080".to_string());
    lines.push("heap_store = \"dir\"".to_string());
    lines.push("heap_dir = \"/var/lib/mcp-v8/heaps\"".to_string());
    lines.push("execution_timeout = 60".to_string());
    lines.push(String::new());
    lines.push("[wasm.math]".to_string());
    lines.push("path = \"/opt/modules/math.wasm\"".to_string());
    lines.push("description = \"Adds two numbers\"".to_string());
    lines.push(String::new());
    lines.push("[[mcp_servers]]".to_string());
    lines.push("name = \"weather\"".to_string());
    lines.push("transport = \"stdio\"".to_string());
    lines.push("command = \"python\"".to_string());
    lines.push("args = [\"server.py\"]".to_string());
    lines.push(String::new());
    lines.push("[[fetch_headers]]".to_string());
    lines.push("host = \"api.github.com\"".to_string());
    lines.push("headers = { Authorization = \"Bearer ...\" }".to_string());
    lines.push(String::new());
    lines.push("[policies.fetch]".to_string());
    lines.push("policies = [{ url = \"file:///etc/mcp-v8/fetch.rego\" }]".to_string());
    lines.push("```".to_string());

    println!("{}", lines.join("\n").trim_end());
}
