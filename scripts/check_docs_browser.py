#!/usr/bin/env python3
"""Serve a built MkDocs site and verify key pages render in headless Chrome."""

from __future__ import annotations

import argparse
import http.server
import os
import shutil
import socketserver
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path


class ReusableTCPServer(socketserver.TCPServer):
    allow_reuse_address = True


def find_chrome(explicit: str | None) -> str:
    candidates = [explicit, os.environ.get("CHROME_BIN"), "google-chrome", "chromium", "chromium-browser"]
    for candidate in candidates:
        if not candidate:
            continue
        resolved = shutil.which(candidate) if os.path.sep not in candidate else candidate
        if resolved and Path(resolved).exists():
            return resolved
    raise SystemExit("Could not find a Chrome/Chromium binary. Set --chrome or CHROME_BIN.")


def wait_for_server(port: int, timeout: float = 10.0) -> None:
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            subprocess.run(
                ["curl", "-sf", f"http://127.0.0.1:{port}/"],
                check=True,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
            return
        except subprocess.CalledProcessError:
            time.sleep(0.2)
    raise SystemExit(f"Docs server on port {port} did not become ready in time.")


def dump_dom(chrome: str, url: str, profile_dir: Path) -> str:
    command = [
        chrome,
        "--headless",
        "--disable-gpu",
        "--disable-dev-shm-usage",
        "--no-sandbox",
        f"--user-data-dir={profile_dir}",
        "--virtual-time-budget=5000",
        "--dump-dom",
        url,
    ]
    result = subprocess.run(
        command,
        check=True,
        capture_output=True,
        text=True,
        timeout=30,
    )
    return result.stdout


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("site_dir", help="Path to the built MkDocs site directory")
    parser.add_argument("--chrome", help="Path to the Chrome/Chromium binary")
    args = parser.parse_args()

    site_dir = Path(args.site_dir).resolve()
    if not site_dir.is_dir():
        raise SystemExit(f"Site directory does not exist: {site_dir}")

    chrome = find_chrome(args.chrome)

    checks = [
        ("/", ["mcp-v8", "Quick Start"]),
        ("/quick-start/overview/", ["Quick Start Overview"]),
        ("/reference/http-api/", ["HTTP API"]),
        ("/reference/mcp-tools/", ["MCP Tools"]),
    ]

    handler = lambda *a, **kw: http.server.SimpleHTTPRequestHandler(*a, directory=str(site_dir), **kw)
    with ReusableTCPServer(("127.0.0.1", 8000), handler) as httpd, tempfile.TemporaryDirectory() as tmpdir:
        thread = threading.Thread(target=httpd.serve_forever, daemon=True)
        thread.start()
        wait_for_server(8000)

        failures: list[str] = []
        for index, (path, expected_strings) in enumerate(checks):
            print(f"Checking {path}", flush=True)
            profile_dir = Path(tmpdir) / f"chromium-profile-{index}"
            profile_dir.mkdir()
            dom = dump_dom(chrome, f"http://127.0.0.1:8000{path}", profile_dir)
            for expected in expected_strings:
                if expected not in dom:
                    failures.append(f"{path}: missing {expected!r}")

        httpd.shutdown()
        thread.join(timeout=5)

    if failures:
        for failure in failures:
            print(failure, file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
