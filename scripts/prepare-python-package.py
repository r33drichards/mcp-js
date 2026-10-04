"""Build-time only: stage generated UniFFI bindings and a native library."""

import argparse
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--library", required=True, type=Path)
    args = parser.parse_args()
    library = args.library.resolve()
    names = {"linux": "libmcp_v8_uniffi.so", "darwin": "libmcp_v8_uniffi.dylib"}
    if sys.platform not in names:
        parser.error("Only Linux and macOS are supported")
    if not library.is_file():
        parser.error(f"Library not found: {library}")
    version = subprocess.check_output(
        ["uniffi-bindgen", "--version"], text=True
    ).strip()
    if version != "uniffi-bindgen 0.32.0":
        parser.error(f"Expected uniffi-bindgen 0.32.0, got {version}")
    package = ROOT / "python" / "mcp_js"
    with tempfile.TemporaryDirectory() as directory:
        subprocess.run(
            [
                "uniffi-bindgen",
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
        shutil.copy2(Path(directory) / "server.py", package / "_bindings.py")
        for native_name in names.values():
            (package / native_name).unlink(missing_ok=True)
        shutil.copy2(library, package / names[sys.platform])
    print(f"Prepared importable package in {package}")


if __name__ == "__main__":
    main()
