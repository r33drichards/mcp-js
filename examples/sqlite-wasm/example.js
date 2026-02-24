// SQLite WASM example for mcp-v8
//
// This script is designed to run inside mcp-v8 with the SQLite WASM module
// pre-loaded via:
//
//   mcp-v8 --stateless --wasm-module sqlite=examples/sqlite-wasm/sqlite3.wasm
//
// The compiled WebAssembly.Module is available as the global `__wasm_sqlite`.
// Because the module has WASI imports, it is NOT auto-instantiated — we
// provide the imports ourselves and instantiate it manually.

// ── WASI stubs ──────────────────────────────────────────────────────────
// SQLite compiled with Emscripten STANDALONE_WASM imports a handful of
// WASI functions. For an in-memory-only database none of these need real
// implementations — simple stubs are sufficient.

var wasiStubs = {
    // fd_close(fd) -> errno
    fd_close: function () { return 0; },
    // fd_fdstat_get(fd, buf) -> errno
    fd_fdstat_get: function () { return 0; },
    // fd_seek(fd, offset_lo, offset_hi, whence, newoffset_ptr) -> errno
    fd_seek: function () { return 0; },
    // fd_write(fd, iovs, iovs_len, nwritten_ptr) -> errno
    fd_write: function (fd, iovs, iovsLen, nwrittenPtr) {
        // Silently discard output (used by sqlite3_log / fprintf)
        var view = new DataView(memory.buffer);
        var totalWritten = 0;
        for (var i = 0; i < iovsLen; i++) {
            var len = view.getUint32(iovs + i * 8 + 4, true);
            totalWritten += len;
        }
        view.setUint32(nwrittenPtr, totalWritten, true);
        return 0;
    },
    // fd_read(fd, iovs, iovs_len, nread_ptr) -> errno
    fd_read: function () { return 0; },
    // environ_get(environ, environ_buf) -> errno
    environ_get: function () { return 0; },
    // environ_sizes_get(count_ptr, buf_size_ptr) -> errno
    environ_sizes_get: function (countPtr, bufSizePtr) {
        var view = new DataView(memory.buffer);
        view.setUint32(countPtr, 0, true);
        view.setUint32(bufSizePtr, 0, true);
        return 0;
    },
    // proc_exit(code) — should never be called
    proc_exit: function () {},
    // clock_time_get(id, precision, time_ptr) -> errno
    clock_time_get: function (id, precLo, precHi, timePtr) {
        var view = new DataView(memory.buffer);
        view.setBigUint64(timePtr, BigInt(0), true);
        return 0;
    },
};

// ── Instantiate the WASM module ─────────────────────────────────────────

var memory; // will be set from the module's exported memory

// Build import object dynamically — enumerate the module's imports and
// provide stubs for anything not explicitly handled above.  This makes
// the example resilient to different Emscripten versions which may add
// or remove WASI / env imports.
var knownImports = {
    wasi_snapshot_preview1: wasiStubs,
    env: {
        emscripten_notify_memory_growth: function () {},
    },
};

var moduleImports = WebAssembly.Module.imports(__wasm_sqlite);
var importObject = {};
for (var i = 0; i < moduleImports.length; i++) {
    var imp = moduleImports[i];
    if (!importObject[imp.module]) {
        importObject[imp.module] = {};
    }
    // Use the known stub if available, otherwise provide a no-op.
    var known = knownImports[imp.module];
    if (known && known[imp.name] !== undefined) {
        importObject[imp.module][imp.name] = known[imp.name];
    } else if (imp.kind === "function") {
        importObject[imp.module][imp.name] = function () { return 0; };
    }
}

var instance = new WebAssembly.Instance(__wasm_sqlite, importObject);

var exports = instance.exports;
memory = exports.memory;

// ── Low-level helpers ───────────────────────────────────────────────────
// These translate between JS strings and WASM linear memory.

function encodeString(str) {
    var bytes = [];
    for (var i = 0; i < str.length; i++) {
        var c = str.charCodeAt(i);
        if (c < 0x80) {
            bytes.push(c);
        } else if (c < 0x800) {
            bytes.push(0xc0 | (c >> 6));
            bytes.push(0x80 | (c & 0x3f));
        } else if (c >= 0xd800 && c <= 0xdbff) {
            // surrogate pair
            var hi = c;
            var lo = str.charCodeAt(++i);
            var cp = ((hi - 0xd800) << 10) + (lo - 0xdc00) + 0x10000;
            bytes.push(0xf0 | (cp >> 18));
            bytes.push(0x80 | ((cp >> 12) & 0x3f));
            bytes.push(0x80 | ((cp >> 6) & 0x3f));
            bytes.push(0x80 | (cp & 0x3f));
        } else {
            bytes.push(0xe0 | (c >> 12));
            bytes.push(0x80 | ((c >> 6) & 0x3f));
            bytes.push(0x80 | (c & 0x3f));
        }
    }
    bytes.push(0); // null terminator
    return bytes;
}

function allocString(str) {
    var bytes = encodeString(str);
    var ptr = exports.malloc(bytes.length);
    if (ptr === 0) throw new Error("malloc failed");
    var view = new Uint8Array(memory.buffer);
    for (var j = 0; j < bytes.length; j++) view[ptr + j] = bytes[j];
    return ptr;
}

function readCString(ptr) {
    if (ptr === 0) return null;
    var view = new Uint8Array(memory.buffer);
    var end = ptr;
    while (view[end] !== 0) end++;
    var bytes = view.slice(ptr, end);
    // Decode UTF-8
    var result = "";
    var i = 0;
    while (i < bytes.length) {
        var b = bytes[i];
        if (b < 0x80) {
            result += String.fromCharCode(b);
            i++;
        } else if (b < 0xe0) {
            result += String.fromCharCode(((b & 0x1f) << 6) | (bytes[i + 1] & 0x3f));
            i += 2;
        } else if (b < 0xf0) {
            result += String.fromCharCode(
                ((b & 0x0f) << 12) | ((bytes[i + 1] & 0x3f) << 6) | (bytes[i + 2] & 0x3f)
            );
            i += 3;
        } else {
            var cp =
                ((b & 0x07) << 18) |
                ((bytes[i + 1] & 0x3f) << 12) |
                ((bytes[i + 2] & 0x3f) << 6) |
                (bytes[i + 3] & 0x3f);
            cp -= 0x10000;
            result += String.fromCharCode(0xd800 + (cp >> 10), 0xdc00 + (cp & 0x3ff));
            i += 4;
        }
    }
    return result;
}

// ── SQLite wrapper ──────────────────────────────────────────────────────

var SQLITE_OK = 0;
var SQLITE_ROW = 100;
var SQLITE_DONE = 101;

// Column type constants
var SQLITE_INTEGER = 1;
var SQLITE_FLOAT = 2;
var SQLITE_TEXT = 3;
var SQLITE_BLOB = 4;
var SQLITE_NULL = 5;

function SQLite() {
    // Allocate a pointer-sized slot for sqlite3_open's output parameter
    this._dbPtrPtr = exports.malloc(4);
    var namePtr = allocString(":memory:");
    var rc = exports.sqlite3_open(namePtr, this._dbPtrPtr);
    exports.free(namePtr);
    if (rc !== SQLITE_OK) {
        throw new Error("sqlite3_open failed: " + rc);
    }
    var view = new DataView(memory.buffer);
    this._db = view.getUint32(this._dbPtrPtr, true);
}

SQLite.prototype.errmsg = function () {
    var ptr = exports.sqlite3_errmsg(this._db);
    return readCString(ptr);
};

SQLite.prototype.exec = function (sql) {
    var sqlPtr = allocString(sql);
    var rc = exports.sqlite3_exec(this._db, sqlPtr, 0, 0, 0);
    exports.free(sqlPtr);
    if (rc !== SQLITE_OK) {
        throw new Error("sqlite3_exec error (" + rc + "): " + this.errmsg());
    }
};

SQLite.prototype.query = function (sql) {
    var sqlPtr = allocString(sql);
    var stmtPtrPtr = exports.malloc(4);
    var rc = exports.sqlite3_prepare_v2(this._db, sqlPtr, -1, stmtPtrPtr, 0);
    exports.free(sqlPtr);
    if (rc !== SQLITE_OK) {
        exports.free(stmtPtrPtr);
        throw new Error("sqlite3_prepare_v2 error (" + rc + "): " + this.errmsg());
    }

    var view = new DataView(memory.buffer);
    var stmt = view.getUint32(stmtPtrPtr, true);
    exports.free(stmtPtrPtr);

    var colCount = exports.sqlite3_column_count(stmt);

    // Read column names
    var columns = [];
    for (var c = 0; c < colCount; c++) {
        var namePtr = exports.sqlite3_column_name(stmt, c);
        columns.push(readCString(namePtr));
    }

    // Read rows
    var rows = [];
    while (true) {
        rc = exports.sqlite3_step(stmt);
        if (rc === SQLITE_DONE) break;
        if (rc !== SQLITE_ROW) {
            exports.sqlite3_finalize(stmt);
            throw new Error("sqlite3_step error (" + rc + "): " + this.errmsg());
        }

        var row = {};
        for (var c = 0; c < colCount; c++) {
            var colType = exports.sqlite3_column_type(stmt, c);
            var name = columns[c];
            if (colType === SQLITE_NULL) {
                row[name] = null;
            } else if (colType === SQLITE_INTEGER) {
                row[name] = exports.sqlite3_column_int(stmt, c);
            } else if (colType === SQLITE_FLOAT) {
                row[name] = exports.sqlite3_column_double(stmt, c);
            } else {
                // TEXT or BLOB — read as string
                var textPtr = exports.sqlite3_column_text(stmt, c);
                row[name] = readCString(textPtr);
            }
        }
        rows.push(row);
    }

    exports.sqlite3_finalize(stmt);
    return { columns: columns, rows: rows };
};

SQLite.prototype.changes = function () {
    return exports.sqlite3_changes(this._db);
};

SQLite.prototype.lastInsertRowId = function () {
    return exports.sqlite3_last_insert_rowid(this._db);
};

SQLite.prototype.close = function () {
    if (this._db) {
        exports.sqlite3_close(this._db);
        exports.free(this._dbPtrPtr);
        this._db = 0;
    }
};

// ── Demo ────────────────────────────────────────────────────────────────

var db = new SQLite();

// Create a table
db.exec("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, email TEXT, age INTEGER)");

// Insert data
db.exec("INSERT INTO users (name, email, age) VALUES ('Alice', 'alice@example.com', 30)");
db.exec("INSERT INTO users (name, email, age) VALUES ('Bob', 'bob@example.com', 25)");
db.exec("INSERT INTO users (name, email, age) VALUES ('Charlie', 'charlie@example.com', 35)");

// Query data
var result = db.query("SELECT * FROM users ORDER BY age");

// Aggregate query
var stats = db.query("SELECT COUNT(*) as count, AVG(age) as avg_age FROM users");

db.close();

JSON.stringify({ users: result.rows, stats: stats.rows[0] });
