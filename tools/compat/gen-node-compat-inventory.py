#!/usr/bin/env python3
"""Generate a deterministic inventory of the pinned Deno Node test corpus."""

from __future__ import annotations

import argparse
import json
import pathlib
import sys

from node_compat_common import load_versions

REPO = pathlib.Path(__file__).resolve().parents[2]
DEFAULT_OUTPUT = REPO / "server/tests/node_compat/inventory.json"
VERSIONS = REPO / "server/tests/node_compat/versions.json"
TEST_DIRS = ("test/parallel", "test/sequential", "test/es-module")
EXTENSIONS = {".js", ".mjs", ".cjs"}


def classify(path: str) -> tuple[str, str, str, str]:
    value = path.lower()
    name = pathlib.PurePosixPath(value).name

    if any(token in value for token in ("addon", "embedding", "node-api", "napi")):
        return "other", "native", "unsupported", "requires native addon or embedding support"
    if "inspector" in value:
        return "other", "inspector", "unsupported", "requires inspector integration"
    if any(token in value for token in ("worker", "message-port", "messageport")):
        return "workers", "workers", "untriaged", "not yet selected for the mcp-v8 runner"
    if any(token in value for token in ("child-process", "child_process", "subprocess", "spawn", "execfile")):
        return "subprocess", "subprocess", "untriaged", "not yet selected for the mcp-v8 runner"
    if any(token in value for token in ("dgram", "udp")):
        return "networking", "network-server" if "server" in value else "network-client", "untriaged", "not yet selected for the mcp-v8 runner"
    if any(token in value for token in ("test-net", "socket", "listen")):
        profile = "network-server" if any(token in value for token in ("server", "listen")) else "network-client"
        return "networking", profile, "untriaged", "not yet selected for the mcp-v8 runner"
    if any(token in value for token in ("test-http", "test-https", "test-http2")):
        profile = "network-server" if any(token in value for token in ("server", "listen")) else "network-client"
        return "http", profile, "untriaged", "not yet selected for the mcp-v8 runner"
    if "test-dns" in value:
        return "dns", "network-client", "untriaged", "not yet selected for the mcp-v8 runner"
    if "test-tls" in value:
        return "tls", "network-client", "untriaged", "not yet selected for the mcp-v8 runner"
    if any(token in value for token in ("test-fs", "filehandle", "read-file", "write-file", "mkdir", "readdir", "stat-")):
        return "filesystem", "filesystem", "untriaged", "not yet selected for the mcp-v8 runner"

    pure_rules = (
        (("test-assert",), "assert"),
        (("test-buffer",), "buffer"),
        (("test-console",), "console"),
        (("test-crypto",), "crypto"),
        (("test-event", "test-events"), "events"),
        (("test-module", "test-require", "test-package", "es-module"), "module"),
        (("test-os",), "os"),
        (("test-path",), "path"),
        (("test-process",), "process"),
        (("test-querystring",), "querystring"),
        (("test-stream",), "streams"),
        (("test-timer", "test-timers"), "timers"),
        (("test-url",), "url"),
        (("test-util",), "util"),
        (("test-vm",), "vm"),
    )
    for tokens, family in pure_rules:
        if any(token in name or token in value for token in tokens):
            return family, "pure", "untriaged", "not yet selected for the mcp-v8 runner"
    return "other", "pure", "untriaged", "not yet selected for the mcp-v8 runner"


def build_inventory(corpus: pathlib.Path, versions_path: pathlib.Path) -> dict:
    versions = load_versions(versions_path)
    source = versions["deno_node_test"]
    tests = []
    for directory in TEST_DIRS:
        root = corpus / directory
        if not root.exists():
            continue
        for path in root.rglob("*"):
            if not path.is_file() or path.suffix not in EXTENSIONS:
                continue
            relative = path.relative_to(corpus).as_posix()
            family, profile, status, reason = classify(relative)
            tests.append(
                {
                    "path": relative,
                    "family": family,
                    "profile": profile,
                    "status": status,
                    "compatibility": "unsupported",
                    "reason": reason,
                }
            )
    tests.sort(key=lambda entry: entry["path"])
    return {
        "schema_version": 1,
        "source": {
            "name": "deno_node_test",
            "commit": source["commit"],
            "node_version": source["node_version"],
        },
        "tests": tests,
    }


def render(data: dict) -> str:
    return json.dumps(data, indent=2, sort_keys=False) + "\n"


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--corpus", type=pathlib.Path, required=True)
    parser.add_argument("--output", type=pathlib.Path, default=DEFAULT_OUTPUT)
    parser.add_argument("--versions", type=pathlib.Path, default=VERSIONS)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()

    content = render(build_inventory(args.corpus, args.versions))
    if args.check:
        if not args.output.exists() or args.output.read_text() != content:
            print(f"inventory drift: regenerate {args.output}", file=sys.stderr)
            return 1
        return 0
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(content)
    print(f"wrote {args.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
