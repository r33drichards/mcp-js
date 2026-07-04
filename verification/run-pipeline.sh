#!/usr/bin/env bash
#
# Reproduce the Charon -> Aeneas Rust->Lean translation for the mcp-js pure
# logic, then run the differential harness that backs the property proofs.
#
# Prerequisites (see verification/README.md for the full story):
#   - Rust (the crate pins its own nightly via rust-toolchain)
#   - A `charon` binary on PATH (built from https://github.com/AeneasVerif/charon)
#   - An `aeneas` binary on PATH (built from https://github.com/AeneasVerif/aeneas;
#     verification/aeneas.nix builds it offline from a pinned nixpkgs)
#
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
CRATE="$HERE/crate"
OUT="$HERE/lean"

echo "==> 1. Differential tests: byte-level model vs verbatim mcp-js originals"
( cd "$CRATE" && cargo test --test differential )

echo "==> 2. Charon: extract LLBC (must use the aeneas preset)"
( cd "$CRATE" && charon cargo --preset=aeneas )

echo "==> 3. Aeneas: translate LLBC -> Lean"
aeneas -backend lean -split-files \
  -dest "$OUT" "$CRATE/mcpjs_verify.llbc"

echo "==> Done. Translated Lean is in $OUT/McpjsVerify/."
echo "    Property statements (prove/disprove workflow output): McpjsVerify/Properties.lean"
