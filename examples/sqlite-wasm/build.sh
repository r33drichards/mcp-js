#!/usr/bin/env bash
set -euo pipefail

# Build SQLite as a standalone WASM module for use with mcp-v8.
#
# Prerequisites:
#   - Emscripten SDK (emsdk) installed and activated
#     https://emscripten.org/docs/getting_started/downloads.html
#
# Output: sqlite3.wasm — a WASI-compatible WASM module that can be loaded
#         with `mcp-v8 --wasm-module sqlite=sqlite3.wasm`

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
BUILD_DIR="$SCRIPT_DIR/build"
SQLITE_VERSION="3490100"
SQLITE_YEAR="2025"
SQLITE_URL="https://www.sqlite.org/${SQLITE_YEAR}/sqlite-amalgamation-${SQLITE_VERSION}.zip"

mkdir -p "$BUILD_DIR"
cd "$BUILD_DIR"

# Download SQLite amalgamation if not already present
if [ ! -f "sqlite3.c" ]; then
    echo "Downloading SQLite amalgamation..."
    curl -fSL "$SQLITE_URL" -o sqlite-amalgamation.zip
    unzip -o sqlite-amalgamation.zip
    cp "sqlite-amalgamation-${SQLITE_VERSION}/sqlite3.c" .
    cp "sqlite-amalgamation-${SQLITE_VERSION}/sqlite3.h" .
    rm -rf "sqlite-amalgamation-${SQLITE_VERSION}" sqlite-amalgamation.zip
fi

echo "Compiling SQLite to WASM..."

emcc sqlite3.c \
    -o sqlite3.wasm \
    -O2 \
    -s STANDALONE_WASM=1 \
    -s EXPORTED_FUNCTIONS='[
        "_sqlite3_open",
        "_sqlite3_close",
        "_sqlite3_exec",
        "_sqlite3_errmsg",
        "_sqlite3_prepare_v2",
        "_sqlite3_step",
        "_sqlite3_column_count",
        "_sqlite3_column_type",
        "_sqlite3_column_int",
        "_sqlite3_column_double",
        "_sqlite3_column_text",
        "_sqlite3_column_bytes",
        "_sqlite3_column_name",
        "_sqlite3_finalize",
        "_sqlite3_changes",
        "_sqlite3_last_insert_rowid",
        "_malloc",
        "_free"
    ]' \
    -s ERROR_ON_UNDEFINED_SYMBOLS=0 \
    -s ALLOW_MEMORY_GROWTH=1 \
    -s INITIAL_MEMORY=16777216 \
    -s STACK_SIZE=65536 \
    -s FILESYSTEM=0 \
    --no-entry \
    -DSQLITE_OS_OTHER=1 \
    -DSQLITE_OMIT_LOAD_EXTENSION \
    -DSQLITE_OMIT_WAL \
    -DSQLITE_THREADSAFE=0 \
    -DSQLITE_OMIT_LOCALTIME \
    -DSQLITE_OMIT_RANDOMNESS \
    -DSQLITE_TEMP_STORE=3 \
    -DSQLITE_OMIT_DEPRECATED

cp sqlite3.wasm "$SCRIPT_DIR/sqlite3.wasm"

echo "Done! Output: $SCRIPT_DIR/sqlite3.wasm"
echo ""
echo "Usage:"
echo "  mcp-v8 --stateless --wasm-module sqlite=examples/sqlite-wasm/sqlite3.wasm"
