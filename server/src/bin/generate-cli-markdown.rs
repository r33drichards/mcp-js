use std::collections::BTreeMap;

use clap::CommandFactory;
use server::cli::Cli;

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

fn main() {
    let command = Cli::command();
    let mut sections: BTreeMap<String, Vec<String>> = BTreeMap::new();

    for arg in command.get_arguments() {
        if arg.is_hide_set() {
            continue;
        }
        let Some(long) = arg.get_long() else {
            continue;
        };

        let heading = arg
            .get_help_heading()
            .map(ToString::to_string)
            .unwrap_or_else(|| "Other".to_string());

        let mut block = Vec::new();
        block.push(format!("### `--{long}`"));
        block.push(String::new());

        let help = arg
            .get_long_help()
            .or_else(|| arg.get_help())
            .map(|value| normalize(&value.to_string()))
            .unwrap_or_default();
        if !help.is_empty() {
            block.push(help);
            block.push(String::new());
        }

        if let Some(env) = arg.get_env() {
            block.push(format!("- Environment: `{}`", env.to_string_lossy()));
        }

        let defaults: Vec<String> = arg
            .get_default_values()
            .iter()
            .map(|value| value.to_string_lossy().to_string())
            .collect();
        if !defaults.is_empty() {
            block.push(format!(
                "- Default: `{}`",
                defaults.join("`, `")
            ));
        }

        if let Some(value_names) = arg.get_value_names() {
            let rendered = value_names
                .iter()
                .flatten()
                .map(|value| value.to_string())
                .collect::<Vec<_>>();
            if !rendered.is_empty() {
                block.push(format!("- Value: `{}`", rendered.join(" ")));
            }
        }

        if let Some(delimiter) = arg.get_value_delimiter() {
            block.push(format!("- Delimiter: `{delimiter}`"));
        }

        let requires = arg
            .get_requires()
            .map(|(id, _)| format!("`--{}`", id.as_str().replace('_', "-")))
            .collect::<Vec<_>>();
        if !requires.is_empty() {
            block.push(format!("- Requires: {}", requires.join(", ")));
        }

        let conflicts = arg
            .get_conflicts()
            .map(|id| format!("`--{}`", id.as_str().replace('_', "-")))
            .collect::<Vec<_>>();
        if !conflicts.is_empty() {
            block.push(format!("- Conflicts: {}", conflicts.join(", ")));
        }

        if block.last().is_some_and(|line| !line.is_empty()) {
            block.push(String::new());
        }

        sections.entry(heading).or_default().push(block.join("\n"));
    }

    let mut lines = vec![
        "# CLI Flags".to_string(),
        String::new(),
        "> Generated from the Clap `Cli` definition. Do not edit this page by hand.".to_string(),
        String::new(),
        "`mcp-v8` is configured through command-line flags. This page is grouped".to_string(),
        "using the same help headings exposed by the CLI itself.".to_string(),
        String::new(),
        "## Sections".to_string(),
        String::new(),
    ];

    for heading in sections.keys() {
        lines.push(format!("- [{}](#{})", heading, slug(heading)));
    }
    lines.push(String::new());

    for (heading, blocks) in sections {
        lines.push(format!("## {heading}"));
        lines.push(String::new());
        for block in blocks {
            lines.push(block);
        }
    }

    println!("{}", lines.join("\n").trim_end());
}
