#!/usr/bin/env bash
set -euo pipefail

jar=${1:?Usage: build_tlc.sh path/to/tla2tools.jar}
# The v1.8.0 prerelease replaces its assets as upstream master changes.
# Build the revision used by the previously verified release binary.
revision=bc1b506bdcad80bb518bf3776a9161c34ff0fc4c
checkout=$(mktemp -d)
trap 'rm -rf -- "$checkout"' EXIT

git -C "$checkout" init --quiet
git -C "$checkout" fetch --quiet --depth=1 --no-tags https://github.com/tlaplus/tlaplus.git "$revision"
git -C "$checkout" checkout --quiet --detach "$revision"
tools_dir="$checkout/tlatools/org.lamport.tlatools"
ant -f "$tools_dir/customBuild.xml" \
  -Dgit.revision="$revision" -Dgit.shortRevision="${revision:0:7}" compile compile-test dist

cp -- "$tools_dir/dist/tla2tools.jar" "$jar"
sha256sum < "$jar" | cut -d ' ' -f 1 > "$jar.sha256"
