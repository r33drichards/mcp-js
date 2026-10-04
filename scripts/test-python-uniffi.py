# /// script
# requires-python = ">=3.10"
# dependencies = []
# ///
"""Build/generate/import the native bindings with `uv run` (no HTTP server)."""

from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def require_tool(name: str) -> str:
    path = shutil.which(name)
    if path is None:
        raise RuntimeError(
            f"{name} is required; run inside `nix develop` (see the bindings guide)"
        )
    return path


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--library", type=Path, help="Existing shared library; skip the Rust/V8 build"
    )
    args = parser.parse_args()
    native_names = {"linux": "libmcp_v8_uniffi.so", "darwin": "libmcp_v8_uniffi.dylib"}
    if sys.platform not in native_names:
        parser.error("The Python shared-library test supports Linux and macOS")
    native_name = native_names[sys.platform]

    try:
        bindgen = require_tool("uniffi-bindgen")
        version = subprocess.check_output([bindgen, "--version"], text=True).strip()
        if version != "uniffi-bindgen 0.32.0":
            raise RuntimeError(f"Expected uniffi-bindgen 0.32.0, found {version}")
        if args.library is not None:
            library = args.library.resolve()
            if not library.is_file():
                raise RuntimeError(f"Shared library not found: {library}")
        else:
            cargo = require_tool("cargo")
            env = os.environ.copy()
            env.pop("RUSTY_V8_ARCHIVE", None)
            env["V8_FROM_SOURCE"] = "1"
            env["GN_ARGS"] = "v8_monolithic=true v8_monolithic_for_shared_library=true"
            # Keep the shared-library V8 build separate from ordinary server builds.
            target = ROOT / "target" / "python-uniffi"
            env["CARGO_TARGET_DIR"] = str(target)
            if env.get("CARGO_BUILD_TARGET"):
                raise RuntimeError(
                    "Unset CARGO_BUILD_TARGET: this test must build for the local Python host"
                )
            subprocess.run(
                [
                    cargo,
                    "build",
                    "-p",
                    "mcp-v8-uniffi-python",
                    "--release",
                    "--config",
                    "mcp-v8-uniffi-python/cargo-config.toml",
                ],
                cwd=ROOT,
                env=env,
                check=True,
            )
            library = target / "release" / native_name
            if not library.is_file():
                raise RuntimeError(f"Build did not produce {library}")

        with tempfile.TemporaryDirectory(prefix="mcp-js-python-") as directory:
            subprocess.run(
                [
                    bindgen,
                    "generate",
                    "--language",
                    "python",
                    "--out-dir",
                    directory,
                    "--no-format",
                    str(library),
                ],
                cwd=ROOT,
                check=True,
            )
            shutil.copy2(library, Path(directory) / native_name)
            env = os.environ.copy()
            env["PYTHONPATH"] = directory
            subprocess.run(
                [
                    sys.executable,
                    str(ROOT / "scripts" / "python-uniffi-library-smoke.py"),
                ],
                cwd=directory,
                env=env,
                check=True,
            )
        return 0
    except (RuntimeError, OSError, subprocess.CalledProcessError) as error:
        print(f"Python UniFFI smoke test failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
