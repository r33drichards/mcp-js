















var wasiStubs = {
    
    fd_close: function () { return 0; },
    
    fd_fdstat_get: function () { return 0; },
    
    fd_seek: function () { return 0; },
    
    fd_write: function (fd, iovs, iovsLen, nwrittenPtr) {
        
        var view = new DataView(memory.buffer);
        var totalWritten = 0;
        for (var i = 0; i < iovsLen; i++) {
            var len = view.getUint32(iovs + i * 8 + 4, true);
            totalWritten += len;
        }
        view.setUint32(nwrittenPtr, totalWritten, true);
        return 0;
    },
    
    fd_read: function () { return 0; },
    
    environ_get: function () { return 0; },
    
    environ_sizes_get: function (countPtr, bufSizePtr) {
        var view = new DataView(memory.buffer);
        view.setUint32(countPtr, 0, true);
        view.setUint32(bufSizePtr, 0, true);
        return 0;
    },
    
    proc_exit: function () {},
    
    clock_time_get: function (id, precLo, precHi, timePtr) {
        var view = new DataView(memory.buffer);
        view.setBigUint64(timePtr, BigInt(0), true);
        return 0;
    },
};



var memory; 





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
    bytes.push(0); 
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



var SQLITE_OK = 0;
var SQLITE_ROW = 100;
var SQLITE_DONE = 101;


var SQLITE_INTEGER = 1;
var SQLITE_FLOAT = 2;
var SQLITE_TEXT = 3;
var SQLITE_BLOB = 4;
var SQLITE_NULL = 5;

function SQLite() {
    
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

    
    var columns = [];
    for (var c = 0; c < colCount; c++) {
        var namePtr = exports.sqlite3_column_name(stmt, c);
        columns.push(readCString(namePtr));
    }

    
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



var db = new SQLite();


db.exec("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, email TEXT, age INTEGER)");


db.exec("INSERT INTO users (name, email, age) VALUES ('Alice', 'alice@example.com', 30)");
db.exec("INSERT INTO users (name, email, age) VALUES ('Bob', 'bob@example.com', 25)");
db.exec("INSERT INTO users (name, email, age) VALUES ('Charlie', 'charlie@example.com', 35)");


var result = db.query("SELECT * FROM users ORDER BY age");


var stats = db.query("SELECT COUNT(*) as count, AVG(age) as avg_age FROM users");

db.close();

console.log(JSON.stringify({ users: result.rows, stats: stats.rows[0] }));
