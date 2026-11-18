#!/usr/bin/env bash
set -euo pipefail

profile="$RUNNER_TEMP/mcp-js-devshell"
sources="$RUNNER_TEMP/mcp-js-flake-sources"
required="$RUNNER_TEMP/mcp-js-nix-paths"
live="$RUNNER_TEMP/mcp-js-nix-live-paths"

# Keeping only the environment lets GC discard flake sources, forcing later
# evaluations to fetch them again. Root the input graph as well as the shell.
nix flake archive --json > "$sources.json"
python3 - "$sources.json" > "$sources" <<'PY'
import json
import sys

paths = set()
def visit(node):
    paths.add(node["path"])
    for child in node.get("inputs", {}).values():
        visit(child)
with open(sys.argv[1]) as source:
    visit(json.load(source))
for path in sorted(paths):
    if not path.startswith("/nix/store/"):
        raise SystemExit(f"Unexpected flake source path: {path}")
    print(path)
PY

index=0
while IFS= read -r source; do
    nix-store --add-root "$RUNNER_TEMP/mcp-js-flake-$index" --indirect --realise "$source" > /dev/null
    index=$((index + 1))
done < "$sources"

nix develop --profile "$profile" --command bash -c '
    rustc_path=$(readlink -f "$(command -v rustc)")
    [[ "$rustc_path" == /nix/store/* ]] || exit 1
    [[ "${RUSTY_V8_ARCHIVE:-}" == /nix/store/* ]] || exit 1
    printf "%s\n" "$rustc_path" "$RUSTY_V8_ARCHIVE"
' > "$required"

# This queries reachability without collecting anything. The root must remain
# alive until the cache action's post-job GC and save have completed.
nix-store --gc --print-live > "$live"
cat "$sources" >> "$required"
while IFS= read -r store_path; do
    grep -Fxq -- "$store_path" "$live" || {
        echo "Required Nix path is not protected from GC: $store_path" >&2
        exit 1
    }
done < <(sed -n 's#^\(/nix/store/[^/]*\).*#\1#p' "$required" | sort -u)

printf 'profile=%s\n' "$profile" >> "$GITHUB_OUTPUT"
printf 'MCP_CI_NIX_PROFILE=%s\n' "$profile" >> "$GITHUB_ENV"
echo "Rooted development shell and $index flake sources; later commands reuse $profile"
