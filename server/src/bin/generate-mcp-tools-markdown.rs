use serde_json::Value;
use server::mcp::{ToolCatalog, built_in_tool_catalog};

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

fn render_description(text: &str) -> Vec<String> {
    text.lines()
        .map(|line| {
            if line.starts_with('#') {
                format!("##{line}")
            } else {
                line.trim_end().to_string()
            }
        })
        .collect()
}

fn schema_type(schema: &Value) -> String {
    let Some(object) = schema.as_object() else {
        return "object".to_string();
    };

    if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
        return reference.rsplit('/').next().unwrap_or(reference).to_string();
    }

    if let Some(one_of) = object.get("oneOf").and_then(Value::as_array) {
        return one_of.iter().map(schema_type).collect::<Vec<_>>().join(" | ");
    }
    if let Some(any_of) = object.get("anyOf").and_then(Value::as_array) {
        return any_of.iter().map(schema_type).collect::<Vec<_>>().join(" | ");
    }
    if let Some(all_of) = object.get("allOf").and_then(Value::as_array) {
        return all_of.iter().map(schema_type).collect::<Vec<_>>().join(" + ");
    }

    let type_label = object
        .get("type")
        .and_then(|value| {
            value.as_str().map(ToString::to_string).or_else(|| {
                value.as_array().map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(" | ")
                })
            })
        })
        .unwrap_or_else(|| "object".to_string());

    let base = match type_label.as_str() {
        "array" => {
            let item = object
                .get("items")
                .map(schema_type)
                .unwrap_or_else(|| "object".to_string());
            format!("array<{item}>")
        }
        "object" => {
            if let Some(value_schema) = object.get("additionalProperties") {
                format!("object<string, {}>", schema_type(value_schema))
            } else {
                "object".to_string()
            }
        }
        other => other.to_string(),
    };

    if let Some(values) = object.get("enum").and_then(Value::as_array) {
        let rendered = values
            .iter()
            .map(|value| format!("`{}`", value))
            .collect::<Vec<_>>()
            .join(", ");
        return format!("{base} ({rendered})");
    }

    base
}

fn table(rows: &[Vec<String>]) -> Vec<String> {
    if rows.is_empty() {
        return Vec::new();
    }
    let mut lines = vec![
        format!("| {} |", rows[0].join(" | ")),
        format!("| {} |", vec!["---"; rows[0].len()].join(" | ")),
    ];
    for row in &rows[1..] {
        lines.push(format!("| {} |", row.join(" | ")));
    }
    lines
}

fn render_params(schema: &Value) -> Vec<String> {
    let Some(object) = schema.as_object() else {
        return Vec::new();
    };
    let Some(properties) = object.get("properties").and_then(Value::as_object) else {
        return Vec::new();
    };
    let required = object
        .get("required")
        .and_then(Value::as_array)
        .map(|items| {
            items.iter()
                .filter_map(Value::as_str)
                .collect::<std::collections::BTreeSet<_>>()
        })
        .unwrap_or_default();

    let mut rows = vec![vec![
        "Parameter".to_string(),
        "Type".to_string(),
        "Required".to_string(),
        "Description".to_string(),
    ]];

    let mut names = properties.keys().cloned().collect::<Vec<_>>();
    names.sort();

    for name in names {
        let property = &properties[&name];
        let description = property
            .get("description")
            .and_then(Value::as_str)
            .map(normalize)
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| "-".to_string());
        rows.push(vec![
            format!("`{name}`"),
            format!("`{}`", schema_type(property)),
            if required.contains(name.as_str()) {
                "yes".to_string()
            } else {
                "no".to_string()
            },
            description,
        ]);
    }

    table(&rows)
}

fn render_mode(catalog: ToolCatalog) -> Vec<String> {
    let mut tools = catalog.tools;
    tools.sort_by(|left, right| left.name.cmp(&right.name));

    let mut lines = vec![
        format!("## {} mode", catalog.mode.to_uppercase().chars().next().unwrap().to_string() + &catalog.mode[1..]),
        String::new(),
        if catalog.mode == "stateful" {
            "These tools keep execution records and heap state available across calls."
                .to_string()
        } else {
            "These tools execute in isolated runs and return output directly."
                .to_string()
        },
        String::new(),
        "### Tools".to_string(),
        String::new(),
    ];

    for tool in &tools {
        lines.push(format!("- [`{}`](#{}-{})", tool.name, catalog.mode, slug(&tool.name)));
    }
    lines.push(String::new());

    for tool in tools {
        lines.push(format!("### `{}`", tool.name));
        lines.push(format!("<a id=\"{}-{}\"></a>", catalog.mode, slug(&tool.name)));
        lines.push(String::new());

        if let Some(description) = tool.description.as_deref() {
            let rendered = render_description(description);
            if rendered.iter().any(|line| !line.is_empty()) {
                lines.extend(rendered);
                lines.push(String::new());
            }
        }

        let params = render_params(&tool.input_schema);
        if params.is_empty() {
            lines.push("This tool does not take structured parameters.".to_string());
            lines.push(String::new());
        } else {
            lines.push("Parameters:".to_string());
            lines.push(String::new());
            lines.extend(params);
            lines.push(String::new());
        }
    }

    lines
}

fn main() {
    // "stateful" doc = the full session-capable surface (heap + fs); "stateless"
    // doc = the no-state surface.
    let stateful = built_in_tool_catalog(true, true);
    let stateless = built_in_tool_catalog(false, false);

    let mut lines = vec![
        "# MCP Tools".to_string(),
        String::new(),
        "> Generated from the built-in MCP tool registry. Do not edit this page by hand.".to_string(),
        String::new(),
        "This page documents the MCP tools exposed by `mcp-v8` itself. Upstream MCP".to_string(),
        "server stubs are not listed here because they depend on your runtime".to_string(),
        "configuration.".to_string(),
        String::new(),
        "## Modes".to_string(),
        String::new(),
        "- [Stateful mode](#stateful-mode)".to_string(),
        "- [Stateless mode](#stateless-mode)".to_string(),
        String::new(),
    ];

    lines.extend(render_mode(stateful));
    lines.push(String::new());
    lines.extend(render_mode(stateless));

    println!("{}", lines.join("\n").trim_end());
}
