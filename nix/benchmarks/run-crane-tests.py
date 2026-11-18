"""Execute installed tests every time; Nix stores compilation, never this verdict."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import sys

artifact = Path(sys.argv[1])
inventory = json.loads((artifact / "test-targets.json").read_text())
with tempfile.TemporaryDirectory(prefix="mcp-crane-tests-") as directory:
    env = os.environ.copy()
    env.update(CARGO_TARGET_TMPDIR=directory, RUST_BACKTRACE="1")
    for target in inventory["targets"]:
        print(f"Running {target['kind']} {target['name']}: {target['executable']}", flush=True)
        subprocess.run([target["executable"], "--test-threads=1"], cwd=target["cwd"], env=env, check=True)
    records = [json.loads(row) for row in (artifact / "doctests.jsonl").read_text().splitlines()]
    if len(records) != 1:
        raise SystemExit(f"Expected one server doctest invocation, found {len(records)}")
    for record in records:
        doc_env = env | record["environment"]
        print("Running server doctests (uncached rustdoc invocation)", flush=True)
        subprocess.run([record["executable"], *record["arguments"], "--test-args=--test-threads=1"],
                       cwd=record["cwd"], env=doc_env, check=True)
