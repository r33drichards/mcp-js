//! Generates the NixOS module's option set (nix/options.nix) from the same
//! sources the `--config` loader runs on: the Clap `Cli` definition,
//! `config::SECTIONS`, and `config::accepted_keys` (the loader's own key
//! vocabulary, which already excludes `REJECTED_KEYS`). Every generated
//! option maps 1:1 to a config-file key; the module renders non-null options
//! to a TOML file passed via `--config`, so the server itself validates the
//! result at startup. The emitted key set is asserted equal to
//! `accepted_keys`, so this file cannot declare an option the loader rejects
//! or omit a key it accepts.

use std::any::TypeId;
use std::collections::BTreeSet;

use clap::ArgAction;
use server::cli::build_command;
use server::config::{SECTIONS, Shape, accepted_keys};

fn normalize(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn help_text(arg: &clap::Arg) -> String {
    let mut text = arg
        .get_long_help()
        .or_else(|| arg.get_help())
        .map(|value| normalize(&value.to_string()))
        .unwrap_or_default();
    // Clap help strings drop the trailing period; restore it so the
    // sentences appended after it read as sentences.
    if !text.is_empty() && !text.ends_with('.') {
        text.push('.');
    }
    text
}

/// Escape `text` into a double-quoted Nix string literal.
fn nix_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            // `${` starts an interpolation inside a double-quoted string.
            '$' if chars.peek() == Some(&'{') => out.push_str("\\$"),
            _ => out.push(ch),
        }
    }
    out.push('"');
    out
}

/// The Nix type of one scalar config value, derived from the arg's own value
/// parser — the exact parser the flag (and therefore the config key) runs on.
fn scalar_nix_type(arg: &clap::Arg) -> String {
    if matches!(arg.get_action(), ArgAction::SetTrue | ArgAction::SetFalse) {
        return "lib.types.bool".to_string();
    }
    let parser = arg.get_value_parser().type_id();
    if parser == TypeId::of::<bool>() {
        return "lib.types.bool".to_string();
    }
    let possible: Vec<String> = arg
        .get_possible_values()
        .iter()
        .map(|value| nix_string(value.get_name()))
        .collect();
    if !possible.is_empty() {
        return format!("lib.types.enum [ {} ]", possible.join(" "));
    }
    if parser == TypeId::of::<u16>() {
        // The only u16 flags are TCP ports.
        return "lib.types.port".to_string();
    }
    let unsigned = [
        TypeId::of::<u8>(),
        TypeId::of::<u32>(),
        TypeId::of::<u64>(),
        TypeId::of::<usize>(),
    ];
    if unsigned.iter().any(|t| parser == *t) {
        return "lib.types.ints.unsigned".to_string();
    }
    let signed = [
        TypeId::of::<i8>(),
        TypeId::of::<i16>(),
        TypeId::of::<i32>(),
        TypeId::of::<i64>(),
        TypeId::of::<isize>(),
    ];
    if signed.iter().any(|t| parser == *t) {
        return "lib.types.int".to_string();
    }
    "lib.types.str".to_string()
}

fn nix_type(arg: &clap::Arg) -> String {
    let scalar = scalar_nix_type(arg);
    if matches!(arg.get_action(), ArgAction::Append) {
        format!("lib.types.listOf {}", parenthesize(&scalar))
    } else {
        scalar
    }
}

fn parenthesize(nix_type: &str) -> String {
    if nix_type.contains(' ') {
        format!("({nix_type})")
    } else {
        nix_type.to_string()
    }
}

/// Description for a flag-backed key: the flag's help plus the facts a module
/// user needs (flag name, env var, server-side default).
fn scalar_description(arg: &clap::Arg, key: &str) -> String {
    let mut text = help_text(arg);
    if !text.is_empty() {
        text.push(' ');
    }
    text.push_str(&format!(
        "Config-file key for the `--{}` flag.",
        arg.get_long().expect("configurable keys have long flags")
    ));
    if let Some(env) = arg.get_env() {
        text.push_str(&format!(
            " Environment variable: `{}`.",
            env.to_string_lossy()
        ));
    }
    // Some Clap defaults are computed at runtime from the host environment;
    // omitting those keeps the generated file deterministic.
    if key != "max_concurrent_executions" {
        let defaults: Vec<String> = arg
            .get_default_values()
            .iter()
            .map(|value| value.to_string_lossy().to_string())
            .collect();
        if !defaults.is_empty() {
            text.push_str(&format!(" Server default: `{}`.", defaults.join("`, `")));
        }
    }
    text
}

fn option_block(key: &str, nix_type: &str, description: &str) -> String {
    format!(
        "  {key} = lib.mkOption {{\n    type = lib.types.nullOr {};\n    default = null;\n    description = {};\n  }};\n",
        parenthesize(nix_type),
        nix_string(description),
    )
}

fn main() {
    let command = build_command();
    let accepted: BTreeSet<String> = accepted_keys(&command).into_iter().collect();

    let mut blocks: Vec<(String, String)> = Vec::new();
    let mut emitted: BTreeSet<String> = BTreeSet::new();

    for section in SECTIONS {
        let target = command
            .get_arguments()
            .find(|arg| arg.get_id().as_str() == section.target_arg)
            .unwrap_or_else(|| {
                panic!(
                    "section '{}' targets unknown arg '{}'",
                    section.key, section.target_arg
                )
            });
        let nix_type = match section.shape {
            Shape::Object => "lib.types.attrsOf lib.types.anything".to_string(),
            Shape::Array => "lib.types.listOf (lib.types.attrsOf lib.types.anything)".to_string(),
        };
        let description = format!(
            "Structured `{}` section of the config file; the inline equivalent of the `--{}` flag \
             (and mutually exclusive with the `{}` key). {}",
            section.key,
            target.get_long().expect("section targets are long flags"),
            section.target_arg,
            help_text(target),
        );
        blocks.push((
            section.key.to_string(),
            option_block(section.key, &nix_type, &description),
        ));
        emitted.insert(section.key.to_string());
    }

    for arg in command.get_arguments() {
        let key = arg.get_id().as_str();
        if !accepted.contains(key) || emitted.contains(key) {
            continue;
        }
        blocks.push((
            key.to_string(),
            option_block(key, &nix_type(arg), &scalar_description(arg, key)),
        ));
        emitted.insert(key.to_string());
    }

    // The generated option set must cover exactly the loader's key vocabulary.
    assert_eq!(
        emitted, accepted,
        "generated Nix options disagree with config::accepted_keys — the generator's filter drifted from the loader's"
    );

    blocks.sort_by(|(a, _), (b, _)| a.cmp(b));

    let mut out = String::new();
    out.push_str(
        "# Do not edit by hand — generated by generate-nix-options from the server's\n\
         # Clap definition and the `--config` loader's section tables. Regenerate with:\n\
         #\n\
         #   cargo run --bin generate-nix-options > nix/options.nix\n\
         #\n\
         # CI's docs-generated-check regenerates and diffs this file, so it cannot\n\
         # drift from the binary. Each option maps 1:1 to a config-file key (see\n\
         # site-docs/reference/config-file.md); `null` (the default everywhere) omits\n\
         # the key from the generated config file, so the server's built-in default\n\
         # and any MCP_V8_* environment variable apply unchanged.\n\
         { lib }:\n\
         {\n",
    );
    for (_, block) in &blocks {
        out.push_str(block);
    }
    out.push_str("}\n");
    print!("{out}");
}
