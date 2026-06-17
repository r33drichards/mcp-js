




{ pkgs }:

pkgs.stdenv.mkDerivation {
  pname = "sqlite3-wasm";
  version = pkgs.sqlite.version;

  
  
  src = pkgs.sqlite.src;

  nativeBuildInputs = [ pkgs.emscripten ];

  buildPhase = ''
    # Emscripten needs a writable cache directory
    export EM_CACHE=$(mktemp -d)

    # Copy the minimal in-memory VFS into the build directory
    # (sqlite3.h is already here from the amalgamation source)
    cp ${../examples/sqlite-wasm/memvfs.c} memvfs.c

    emcc sqlite3.c memvfs.c \
      -o sqlite3.wasm \
      -O2 \
      -sSTANDALONE_WASM=1 \
      -sEXPORTED_FUNCTIONS='[
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
      -sERROR_ON_UNDEFINED_SYMBOLS=0 \
      -sALLOW_MEMORY_GROWTH=1 \
      -sINITIAL_MEMORY=16777216 \
      -sSTACK_SIZE=65536 \
      -sFILESYSTEM=0 \
      --no-entry \
      -DSQLITE_OS_OTHER=1 \
      -DSQLITE_OMIT_LOAD_EXTENSION \
      -DSQLITE_OMIT_WAL \
      -DSQLITE_THREADSAFE=0 \
      -DSQLITE_OMIT_LOCALTIME \
      -DSQLITE_OMIT_RANDOMNESS \
      -DSQLITE_TEMP_STORE=3 \
      -DSQLITE_OMIT_DEPRECATED
  '';

  installPhase = ''
    mkdir -p $out
    cp sqlite3.wasm $out/sqlite3.wasm
  '';
}
