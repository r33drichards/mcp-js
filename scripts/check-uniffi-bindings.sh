#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out_root="$(mktemp -d)"
trap 'rm -rf "$out_root"' EXIT

require_symbol() {
  local file="$1"
  local symbol="$2"
  if ! grep -Fq "$symbol" "$file"; then
    echo "$file is missing generated symbol: $symbol" >&2
    exit 1
  fi
}

for language in swift kotlin python ruby; do
  "$repo_root/scripts/generate-uniffi-bindings.sh" "$language" "$out_root/$language"
done

swift_file="$out_root/swift/server.swift"
require_symbol "$swift_file" 'class Engine'
require_symbol "$swift_file" 'struct McpRequestHeaders'

kotlin_file="$out_root/kotlin/uniffi/server/server.kt"
require_symbol "$kotlin_file" 'class Engine'
require_symbol "$kotlin_file" 'data class McpRequestHeaders'

python_file="$out_root/python/server.py"
require_symbol "$python_file" 'class Engine'
require_symbol "$python_file" 'class McpRequestHeaders'

ruby_file="$out_root/ruby/server.rb"
require_symbol "$ruby_file" 'class Engine'
require_symbol "$ruby_file" 'class McpRequestHeaders'

if grep -R -Fq 'mcpHeadersJson' "$out_root"; then
  echo "Generated bindings still expose the removed JSON header field" >&2
  exit 1
fi

echo "UniFFI binding smoke checks passed for Swift, Kotlin, Python, and Ruby"
