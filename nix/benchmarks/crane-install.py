"""Install Cargo's exact discovered executable inventory, without running it."""
import json
import os
from pathlib import Path
import sys

log, output, source, target = map(Path, sys.argv[1:])
target = target.resolve()
output.mkdir(exist_ok=True)
(output / "bin").mkdir(exist_ok=True)
records = []
examples = []
for line in log.read_text().splitlines():
    try:
        item = json.loads(line)
    except json.JSONDecodeError:
        continue
    if item.get("reason") != "compiler-artifact" or not item.get("executable"):
        continue
    executable = Path(item["executable"])
    target_meta = item["target"]
    relative = executable.relative_to(target)
    installed = str(output / "target" / relative)
    if item["profile"]["test"]:
        records.append({"name": target_meta["name"], "kind": target_meta["kind"],
                        "executable": installed, "cwd": str(source / "server")})
    elif "bin" in target_meta["kind"]:
        link = output / "bin" / target_meta["name"]
        if not link.exists():
            link.symlink_to(Path("../target") / relative)
    elif "example" in target_meta["kind"]:
        examples.append({"name": target_meta["name"], "path": installed})
# Match Cargo's target execution order; a lib and bin may share their name.
order = {"lib": 0, "bin": 1, "test": 2}
records.sort(key=lambda row: (min(order.get(k, 3) for k in row["kind"]), row["name"]))
expected = json.loads((source / "scripts/ci/expected-nix-comparison-tests.json").read_text())
expected_targets = sorted((row["name"], tuple(row["kind"])) for row in expected["targets"]
                          if row["test"] and "example" not in row["kind"])
actual_targets = sorted((row["name"], tuple(row["kind"])) for row in records)
if actual_targets != expected_targets:
    raise SystemExit(f"Cargo target inventory differs: actual={actual_targets!r}, expected={expected_targets!r}")
expected_examples = {row["name"] for row in expected["targets"] if "example" in row["kind"]}
if {row["name"] for row in examples} != expected_examples:
    raise SystemExit("Compile-only example inventory differs from baseline")
(output / "test-targets.json").write_text(json.dumps({
    "framework": "crane", "baseline_commit": expected["baseline_commit"],
    "expected_passed": expected["baseline_passed"], "expected_ignored": expected["baseline_ignored"],
    "targets": records, "compiled_examples": sorted(examples, key=lambda row: row["name"]),
}, indent=2) + "\n")

(output / "share").mkdir(exist_ok=True)
(output / "share/benchmark-targets.json").write_text(json.dumps({
    "targets": [{"name": row["name"], "kind": row["kind"][0], "path": row["executable"]} for row in records],
    "example_targets": sorted(examples, key=lambda row: row["name"]),
    "doctests": True,
}, indent=2) + "\n")
