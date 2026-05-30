#!/usr/bin/env python3
"""Generate MkDocs-friendly HTTP API reference Markdown from openapi.json."""

from __future__ import annotations

import argparse
import json
from pathlib import Path


METHOD_ORDER = ["get", "post", "put", "patch", "delete", "options", "head"]


def slug(text: str) -> str:
    out = []
    for ch in text.lower():
        if ch.isalnum():
            out.append(ch)
        elif ch in {" ", "-", "_", "/", "{", "}", "."}:
            out.append("-")
    collapsed = "".join(out)
    while "--" in collapsed:
        collapsed = collapsed.replace("--", "-")
    return collapsed.strip("-")


def load_json(path: Path) -> dict:
    with path.open("r", encoding="utf-8") as handle:
        return json.load(handle)


def schema_type(schema: dict, ref_links: bool = True) -> str:
    if not schema:
        return "object"

    ref = schema.get("$ref")
    if ref:
        name = ref.split("/")[-1]
        if ref_links:
            return f"[`{name}`](#schema-{slug(name)})"
        return name

    if "oneOf" in schema:
        return " | ".join(schema_type(item, ref_links=ref_links) for item in schema["oneOf"])
    if "anyOf" in schema:
        return " | ".join(schema_type(item, ref_links=ref_links) for item in schema["anyOf"])
    if "allOf" in schema:
        return " + ".join(schema_type(item, ref_links=ref_links) for item in schema["allOf"])

    typ = schema.get("type", "object")
    if typ == "array":
        item = schema_type(schema.get("items", {}), ref_links=ref_links)
        label = f"array<{item}>"
    elif typ == "object":
        if "additionalProperties" in schema:
            value = schema_type(schema["additionalProperties"], ref_links=ref_links)
            label = f"object<string, {value}>"
        else:
            label = "object"
    else:
        label = typ

    enum = schema.get("enum")
    if enum:
        label += " (" + ", ".join(f"`{value}`" for value in enum) + ")"
    if schema.get("nullable"):
        label += " | null"
    return label


def collect_refs_from_schema(schema: dict, refs: set[str]) -> None:
    if not isinstance(schema, dict):
        return

    ref = schema.get("$ref")
    if ref and ref.startswith("#/components/schemas/"):
        refs.add(ref.split("/")[-1])
        return

    for key in ("oneOf", "anyOf", "allOf"):
        for item in schema.get(key, []):
            collect_refs_from_schema(item, refs)

    if "items" in schema:
        collect_refs_from_schema(schema["items"], refs)

    if "properties" in schema:
        for item in schema["properties"].values():
            collect_refs_from_schema(item, refs)

    if "additionalProperties" in schema:
        collect_refs_from_schema(schema["additionalProperties"], refs)


def collect_operation_refs(operation: dict, refs: set[str]) -> None:
    for parameter in operation.get("parameters", []):
        collect_refs_from_schema(parameter.get("schema", {}), refs)

    request_body = operation.get("requestBody", {})
    for content in request_body.get("content", {}).values():
        collect_refs_from_schema(content.get("schema", {}), refs)

    for response in operation.get("responses", {}).values():
        for content in response.get("content", {}).values():
            collect_refs_from_schema(content.get("schema", {}), refs)


def collect_nested_component_refs(name: str, schemas: dict, refs: set[str], seen: set[str]) -> None:
    if name in seen:
        return
    seen.add(name)
    schema = schemas.get(name, {})
    nested = set()
    collect_refs_from_schema(schema, nested)
    for child in nested:
        if child not in refs:
            refs.add(child)
        collect_nested_component_refs(child, schemas, refs, seen)


def render_table(rows: list[list[str]]) -> list[str]:
    if not rows:
        return []
    lines = [
        "| " + " | ".join(rows[0]) + " |",
        "| " + " | ".join("---" for _ in rows[0]) + " |",
    ]
    for row in rows[1:]:
        lines.append("| " + " | ".join(row) + " |")
    return lines


def render_operation(method: str, path: str, operation: dict) -> list[str]:
    lines = [f"## `{method.upper()} {path}`", ""]

    summary = operation.get("summary")
    if summary:
        lines.append(summary)
        lines.append("")

    description = operation.get("description")
    if description:
        lines.append(description.replace("\n", " "))
        lines.append("")

    tags = operation.get("tags", [])
    if tags:
        lines.append("Tags: " + ", ".join(f"`{tag}`" for tag in tags))
        lines.append("")

    parameters = operation.get("parameters", [])
    if parameters:
        lines.append("### Parameters")
        lines.append("")
        rows = [["Name", "In", "Required", "Type", "Description"]]
        for parameter in parameters:
            rows.append(
                [
                    f"`{parameter['name']}`",
                    f"`{parameter.get('in', '')}`",
                    "yes" if parameter.get("required") else "no",
                    schema_type(parameter.get("schema", {})),
                    " ".join((parameter.get("description") or "").splitlines()) or "-",
                ]
            )
        lines.extend(render_table(rows))
        lines.append("")

    request_body = operation.get("requestBody")
    if request_body:
        lines.append("### Request Body")
        lines.append("")
        if request_body.get("required"):
            lines.append("Required.")
            lines.append("")
        rows = [["Content Type", "Schema"]]
        for content_type, content in sorted(request_body.get("content", {}).items()):
            rows.append([f"`{content_type}`", schema_type(content.get("schema", {}))])
        lines.extend(render_table(rows))
        lines.append("")

    responses = operation.get("responses", {})
    if responses:
        lines.append("### Responses")
        lines.append("")
        rows = [["Status", "Content Type", "Schema", "Description"]]
        for status, response in responses.items():
            content = response.get("content", {})
            description = " ".join((response.get("description") or "").splitlines()) or "-"
            if content:
                first = True
                for content_type, body in sorted(content.items()):
                    rows.append(
                        [
                            f"`{status}`" if first else "",
                            f"`{content_type}`",
                            schema_type(body.get("schema", {})),
                            description if first else "",
                        ]
                    )
                    first = False
            else:
                rows.append([f"`{status}`", "-", "-", description])
        lines.extend(render_table(rows))
        lines.append("")

    return lines


def render_schema(name: str, schema: dict) -> list[str]:
    lines = [f"## Schema `{name}`", ""]

    description = schema.get("description")
    if description:
        lines.append(description.replace("\n", " "))
        lines.append("")

    lines.append(f"Type: `{schema_type(schema, ref_links=False)}`")
    lines.append("")

    required = set(schema.get("required", []))
    properties = schema.get("properties", {})
    if properties:
        rows = [["Property", "Type", "Required", "Description"]]
        for prop_name, prop_schema in properties.items():
            rows.append(
                [
                    f"`{prop_name}`",
                    schema_type(prop_schema),
                    "yes" if prop_name in required else "no",
                    " ".join((prop_schema.get("description") or "").splitlines()) or "-",
                ]
            )
        lines.extend(render_table(rows))
        lines.append("")
    elif "items" in schema:
        lines.append(f"Items: {schema_type(schema['items'])}")
        lines.append("")
    elif "additionalProperties" in schema:
        lines.append(f"Additional properties: {schema_type(schema['additionalProperties'])}")
        lines.append("")

    return lines


def generate_markdown(spec: dict) -> str:
    info = spec.get("info", {})
    paths = spec.get("paths", {})
    components = spec.get("components", {})
    schemas = components.get("schemas", {})

    lines: list[str] = [
        "# HTTP API",
        "",
        "> Generated from the repo root `openapi.json`. Do not edit this page by hand.",
        "",
        "When `mcp-v8` runs with `--http-port` or `--sse-port`, it exposes a plain HTTP",
        "API alongside MCP transport.",
        "",
        f"OpenAPI version: `{spec.get('openapi', '')}`",
        "",
        f"API title: `{info.get('title', '')}`",
        "",
        f"API version: `{info.get('version', '')}`",
        "",
        "## Endpoints",
        "",
    ]

    summary_rows = [["Method", "Path", "Summary"]]
    operations: list[tuple[str, str, dict]] = []
    for path in sorted(paths.keys()):
        path_item = paths[path]
        for method in METHOD_ORDER:
            if method in path_item:
                operation = path_item[method]
                operations.append((method, path, operation))
                summary_rows.append(
                    [f"`{method.upper()}`", f"`{path}`", operation.get("summary", "-")]
                )
    lines.extend(render_table(summary_rows))
    lines.append("")

    used_refs: set[str] = set()
    for method, path, operation in operations:
        lines.extend(render_operation(method, path, operation))
        collect_operation_refs(operation, used_refs)

    if used_refs:
        expanded_refs = set(used_refs)
        for name in sorted(list(used_refs)):
            collect_nested_component_refs(name, schemas, expanded_refs, set())

        lines.append("## Component Schemas")
        lines.append("")
        for name in sorted(expanded_refs):
            schema = schemas.get(name)
            if schema:
                lines.extend(render_schema(name, schema))

    return "\n".join(lines).rstrip() + "\n"


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--input", default="openapi.json")
    parser.add_argument("--output", default="site-docs/reference/http-api.md")
    args = parser.parse_args()

    spec = load_json(Path(args.input))
    markdown = generate_markdown(spec)
    Path(args.output).write_text(markdown, encoding="utf-8")


if __name__ == "__main__":
    main()
