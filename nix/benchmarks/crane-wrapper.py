"""Keep paths embedded by Cargo usable after the Nix build sandbox disappears."""
import json
import os
from pathlib import Path
import sys

mode, *arguments = sys.argv[1:]
env = os.environ.copy()
build_root = env.get("BENCHMARK_BUILD_ROOT")
source = env.get("BENCHMARK_RUNTIME_SOURCE")
output = env.get("BENCHMARK_RUNTIME_OUTPUT")
target = env.get("CARGO_TARGET_DIR", "target")
if build_root and not os.path.isabs(target):
    target = str(Path(build_root) / target)


def relocate(value):
    if output and target:
        value = value.replace(target, str(Path(output) / "target"))
    if source and build_root:
        value = value.replace(build_root, source)
    return value


if mode == "rustc":
    rustc, *arguments = arguments
    if source and build_root:
        manifest = env.get("CARGO_MANIFEST_DIR", "")
        if manifest.startswith(build_root + "/"):
            env["CARGO_MANIFEST_DIR"] = relocate(manifest)
        for key, value in list(env.items()):
            if key.startswith("CARGO_BIN_EXE_"):
                env[key] = str(Path(output) / "bin" / Path(value).name)
    os.execvpe(rustc, [rustc, *arguments], env)
elif mode == "rustdoc":
    rustdoc = env["BENCHMARK_REAL_RUSTDOC"]
    if "--test" not in arguments or not output:
        os.execvpe(rustdoc, [rustdoc, *arguments], env)
    # Capture the command, not a successful/cached test verdict. The external
    # runner executes this exact rustdoc invocation on every benchmark trial.
    record = {
        "executable": rustdoc,
        "arguments": [relocate(value) for value in arguments],
        "cwd": relocate(os.getcwd()),
        "environment": {
            key: relocate(value) for key, value in env.items()
            if key.startswith(("CARGO_PKG_", "CARGO_BIN_EXE_")) or key in ("CARGO_MANIFEST_DIR", "OUT_DIR")
        },
    }
    with open(Path(output) / "doctests.jsonl", "a") as destination:
        destination.write(json.dumps(record) + "\n")
else:
    raise SystemExit("unknown wrapper mode")
