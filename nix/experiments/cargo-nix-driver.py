#!/usr/bin/env python3
"""Durable fixture paths and uncached doctest replay for the cargo-nix trial.

This is an experiment adapter, not an upstream plugin patch. Normal compilation
uses the selected rustc. Rustdoc's exact doctest command is retained for the
outside-sandbox runner instead of executing tests inside a cached derivation.
"""
import json
import os
import re
from pathlib import Path
import shutil
import subprocess
import sys

KEEP = {"OUT_DIR", "PATH", "HOST", "TARGET", "DEBUG", "OPT_LEVEL", "PROFILE",
        "LD_LIBRARY_PATH", "LIBRARY_PATH", "PKG_CONFIG_PATH", "NIX_LDFLAGS",
        "NIX_CFLAGS_COMPILE", "NIX_CC", "NIX_BINTOOLS", "RUSTFLAGS", "RUSTDOCFLAGS",
        "SOURCE_DATE_EPOCH"}


def captured_env():
    return {k: v for k, v in os.environ.items()
            if k in KEEP or k.startswith(("CARGO_", "DEP_"))}


def output_path():
    attrs = json.loads(Path(os.environ["NIX_ATTRS_JSON_FILE"]).read_text())
    return Path(attrs["outputs"]["out"])


def manifest_env(source):
    os.environ["CARGO_MANIFEST_DIR"] = str(Path(source) / "server")
    os.environ["CARGO_MANIFEST_PATH"] = str(Path(source) / "server/Cargo.toml")


def rustc(real, source, args):
    manifest_env(source)
    if os.environ.get("CARGO_BIN_NAME") == "server" and "--test" not in args:
        Path("target").mkdir(exist_ok=True)
        Path("target/benchmark-example-command.json").write_text(json.dumps({
            "compiler": real, "args": args, "env": captured_env(), "source": source,
        }))
    os.execv(real, [real, *args])


def rustdoc(real, source, args):
    manifest_env(source)
    if "--test" not in args:
        os.execv(real, [real, *args])
    out = output_path()
    support = out / "nix-support"
    support.mkdir(parents=True, exist_ok=True)
    retained = out / "doctest-target"
    retained.mkdir(exist_ok=True)
    for name in ["deps", "build", "buildDeps"]:
        directory = Path("target") / name
        if directory.exists():
            shutil.copytree(directory, retained / name, symlinks=True, dirs_exist_ok=True)
    (retained / "lib").mkdir(exist_ok=True)
    for artifact in Path("target/lib").iterdir():
        if artifact.suffix in {".rlib", ".rmeta", ".so", ".a", ".dylib"}:
            shutil.copy2(artifact, retained / "lib" / artifact.name)
    cwd = str(Path.cwd())

    def durable(value):
        value = value.replace(cwd + "/target/", str(retained) + "/")
        value = re.sub(r"(^|=)target/", lambda m: m.group(1) + str(retained) + "/", value)
        return value.replace(cwd, str(Path(source) / "server"))

    doc_args = [durable(a) for a in args]
    doc_args = [str(Path(source) / "server" / a) if a == "src/lib.rs" else a
                for a in doc_args]
    env = {key: durable(value) for key, value in captured_env().items()}
    (support / "benchmark-doctest.json").write_text(json.dumps({
        "program": real, "args": doc_args, "env": env,
        "cwd": str(Path(source) / "server"),
    }, indent=2))
    print("Captured rustdoc --test for uncached runtime replay", file=sys.stderr)


def compile_example():
    recipe = json.loads(Path("target/benchmark-example-command.json").read_text())
    args = recipe["args"][:]
    args[args.index("--crate-name") + 1] = "node_compat_full"
    inputs = [i for i, arg in enumerate(args) if arg == "src/main.rs" or arg.endswith("/src/main.rs")]
    if len(inputs) != 1:
        raise SystemExit("Cannot identify the server input for example parity compilation")
    args[inputs[0]] = str(Path(recipe["source"]) / "server/examples/node_compat_full.rs")
    env = dict(os.environ, **recipe["env"], CARGO_BIN_NAME="node_compat_full")
    subprocess.run([recipe["compiler"], *args], env=env, check=True)


def replay(recipe_path):
    recipe = json.loads(Path(recipe_path).read_text())
    env = dict(os.environ, **recipe["env"])
    env["CARGO_TARGET_TMPDIR"] = os.environ["CARGO_TARGET_TMPDIR"]
    subprocess.run([recipe["program"], *recipe["args"], "--test-args=--test-threads=1"],
                   cwd=recipe["cwd"], env=env, check=True)


def write_manifest():
    out = output_path()
    metadata = json.loads((out / "nix-support/binaries-metadata.json").read_text())
    targets = [{"name": target["binary-name"], "kind": target["kind"],
                "path": target["binary-path"]}
               for target in metadata["rust-binaries"].values()]
    targets.sort(key=lambda target: ({"lib": 0, "bin": 1, "test": 2}[target["kind"]], target["name"]))
    example = out / "bin/node_compat_full"
    assert example.is_file(), "Cargo's compile-only example was not built"
    assert (out / "nix-support/benchmark-doctest.json").is_file(), "Missing doctest recipe"
    for target in targets:
        assert Path(target["path"]).is_file(), target
    share = out / "share"
    share.mkdir(exist_ok=True)
    (share / "benchmark-targets.json").write_text(json.dumps({
        "targets": targets, "example_targets": [{"name": "node_compat_full", "path": str(example)}],
        "doctests": True,
    }, indent=2))


def run_tests(output):
    output = Path(output)
    manifest = json.loads((output / "share/benchmark-targets.json").read_text())
    for target in manifest["targets"]:
        print("── running " + target["kind"] + ":" + target["name"], flush=True)
        subprocess.run([target["path"], "--test-threads=1"], check=True)
    replay(output / "nix-support/benchmark-doctest.json")


def main():
    mode = sys.argv[1]
    if mode in {"rustc", "rustdoc"}:
        {"rustc": rustc, "rustdoc": rustdoc}[mode](sys.argv[2], sys.argv[3], sys.argv[4:])
    elif mode == "example":
        compile_example()
    elif mode == "replay":
        replay(sys.argv[2])
    elif mode == "manifest":
        write_manifest()
    elif mode == "run":
        run_tests(sys.argv[2])
    else:
        raise SystemExit("Unknown driver mode")


if __name__ == "__main__":
    main()
