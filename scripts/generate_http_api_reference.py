#!/usr/bin/env python3
"""Generate MkDocs-friendly HTTP API reference Markdown from openapi.json."""

from __future__ import annotations

import argparse
import re
import shutil
import subprocess
import tempfile
from pathlib import Path


INTRO = [
    "# HTTP API",
    "",
    "> Generated from the repo root `openapi.json` with Widdershins. Do not edit",
    "> this page by hand.",
    "",
    "This page documents the HTTP surface exposed by `mcp-v8` from the committed",
    "OpenAPI description.",
    "",
]


TOP_LEVEL_HEADING = re.compile(r'^<h1 id="[^"]+">([^<]+)</h1>$')
THIRD_LEVEL_HEADING = re.compile(r'^<h3 id="([^"]+)">([^<]+)</h3>$')


def run_widdershins(input_path: Path) -> str:
    repo_root = Path(__file__).resolve().parent.parent
    widdershins_cli = repo_root / "tools" / "widdershins" / "node_modules" / "widdershins" / "widdershins.js"
    if not widdershins_cli.exists():
        raise SystemExit(f"Missing vendored Widdershins CLI at {widdershins_cli}")

    node = shutil.which("node")
    if not node:
        raise SystemExit("Could not find `node` in PATH.")

    with tempfile.NamedTemporaryFile("r+", encoding="utf-8", suffix=".md", delete=False) as handle:
        tmp_output = Path(handle.name)

    command = [
        node,
        str(widdershins_cli),
        "--omitHeader",
        "--summary",
        "-c",
        str(input_path),
        "-o",
        str(tmp_output),
    ]
    subprocess.run(command, check=True)
    try:
        return tmp_output.read_text(encoding="utf-8")
    finally:
        tmp_output.unlink(missing_ok=True)


def normalize_heading(text: str) -> str:
    words = text.replace("-", " ").split()
    if len(words) == 1 and words[0].lower() == "cli":
        return "CLI"
    return " ".join(word.upper() if word.isupper() else word.capitalize() for word in words)


def transform(text: str) -> str:
    lines = text.splitlines()
    output: list[str] = list(INTRO)
    in_intro = True
    in_auth_aside = False
    auth_lines: list[str] = []

    for line in lines:
        stripped = line.strip()

        if stripped.startswith("<!-- Generator:"):
            continue

        if in_auth_aside:
            if stripped == "</aside>":
                auth_text = " ".join(item.strip() for item in auth_lines if item.strip())
                if auth_text == "This operation does not require authentication":
                    output.append("Authentication: none.")
                    output.append("")
                else:
                    output.append(auth_text)
                    output.append("")
                in_auth_aside = False
                auth_lines.clear()
            else:
                auth_lines.append(stripped)
            continue

        if stripped == "<aside class=\"success\">":
            in_auth_aside = True
            auth_lines.clear()
            continue

        top_match = TOP_LEVEL_HEADING.match(stripped)
        if top_match:
            heading = top_match.group(1)
            if in_intro:
                if heading.lower().startswith("mcp-v8 v"):
                    continue
                in_intro = False
            output.append(f"## {normalize_heading(heading)}")
            output.append("")
            continue

        if in_intro:
            continue

        third_match = THIRD_LEVEL_HEADING.match(stripped)
        if third_match:
            output.append(f'<a id="{third_match.group(1)}"></a>')
            output.append(f"#### {third_match.group(2)}")
            output.append("")
            continue

        if stripped.startswith("<h2>") and stripped.endswith("</h2>"):
            output.append(f"### {stripped[4:-5]}")
            output.append("")
            continue

        if stripped.startswith("## "):
            output.append(f"### {stripped[3:]}")
            output.append("")
            continue

        if stripped == "License:":
            continue

        output.append(line)

    while output and not output[-1].strip():
        output.pop()
    output.append("")
    return "\n".join(output)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--input", default="openapi.json")
    parser.add_argument("--output", default="site-docs/reference/http-api.md")
    args = parser.parse_args()

    rendered = run_widdershins(Path(args.input))
    Path(args.output).write_text(transform(rendered), encoding="utf-8")


if __name__ == "__main__":
    main()
