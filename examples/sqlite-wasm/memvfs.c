/*
 * Minimal in-memory VFS for SQLite WASM builds using SQLITE_OS_OTHER=1.
 *
 * Provides sqlite3_os_init() / sqlite3_os_end() which register a bare-minimum
 * VFS that supports :memory: databases and temporary files stored in memory.
 * No real filesystem access is performed.
 */
#include "sqlite3.h"
#include <string.h>
#include <stdlib.h>

/* ── In-memory file ────────────────────────────────────────────────── */

typedef struct MemFile {
    sqlite3_file base;
    char *buf;
    int sz;
    int cap;
} MemFile;

static int memClose(sqlite3_file *pFile) {
    MemFile *p = (MemFile *)pFile;
    free(p->buf);
    p->buf = NULL;
    p->sz = 0;
    p->cap = 0;
    return SQLITE_OK;
}

static int memRead(sqlite3_file *pFile, void *zBuf, int iAmt, sqlite3_int64 iOfst) {
    MemFile *p = (MemFile *)pFile;
    int avail = p->sz - (int)iOfst;
    if (avail <= 0) {
        memset(zBuf, 0, iAmt);
        return SQLITE_IOERR_SHORT_READ;
    }
    if (avail < iAmt) {
        memcpy(zBuf, p->buf + iOfst, avail);
        memset((char *)zBuf + avail, 0, iAmt - avail);
        return SQLITE_IOERR_SHORT_READ;
    }
    memcpy(zBuf, p->buf + iOfst, iAmt);
    return SQLITE_OK;
}

static int memWrite(sqlite3_file *pFile, const void *zBuf, int iAmt, sqlite3_int64 iOfst) {
    MemFile *p = (MemFile *)pFile;
    int needed = (int)iOfst + iAmt;
    if (needed > p->cap) {
        int newCap = p->cap ? p->cap : 4096;
        while (newCap < needed) newCap *= 2;
        char *newBuf = (char *)realloc(p->buf, newCap);
        if (!newBuf) return SQLITE_NOMEM;
        /* Zero the gap between old size and the write offset */
        if ((int)iOfst > p->sz) {
            memset(newBuf + p->sz, 0, (int)iOfst - p->sz);
        }
        p->buf = newBuf;
        p->cap = newCap;
    }
    memcpy(p->buf + iOfst, zBuf, iAmt);
    if (needed > p->sz) p->sz = needed;
    return SQLITE_OK;
}

static int memTruncate(sqlite3_file *pFile, sqlite3_int64 size) {
    MemFile *p = (MemFile *)pFile;
    if ((int)size < p->sz) p->sz = (int)size;
    return SQLITE_OK;
}

static int memSync(sqlite3_file *pFile, int flags) {
    (void)pFile; (void)flags;
    return SQLITE_OK;
}

static int memFileSize(sqlite3_file *pFile, sqlite3_int64 *pSize) {
    *pSize = ((MemFile *)pFile)->sz;
    return SQLITE_OK;
}

static int memLock(sqlite3_file *f, int l) { (void)f; (void)l; return SQLITE_OK; }
static int memUnlock(sqlite3_file *f, int l) { (void)f; (void)l; return SQLITE_OK; }
static int memCheckReservedLock(sqlite3_file *f, int *pOut) { (void)f; *pOut = 0; return SQLITE_OK; }
static int memFileControl(sqlite3_file *f, int op, void *a) { (void)f; (void)op; (void)a; return SQLITE_NOTFOUND; }
static int memSectorSize(sqlite3_file *f) { (void)f; return 512; }
static int memDeviceCharacteristics(sqlite3_file *f) { (void)f; return 0; }

static const sqlite3_io_methods memIoMethods = {
    1,                          /* iVersion */
    memClose,
    memRead,
    memWrite,
    memTruncate,
    memSync,
    memFileSize,
    memLock,
    memUnlock,
    memCheckReservedLock,
    memFileControl,
    memSectorSize,
    memDeviceCharacteristics,
};

/* ── VFS methods ───────────────────────────────────────────────────── */

static int memVfsOpen(sqlite3_vfs *pVfs, const char *zName,
                      sqlite3_file *pFile, int flags, int *pOutFlags) {
    (void)pVfs; (void)zName; (void)flags;
    MemFile *p = (MemFile *)pFile;
    memset(p, 0, sizeof(*p));
    p->base.pMethods = &memIoMethods;
    if (pOutFlags) *pOutFlags = flags;
    return SQLITE_OK;
}

static int memVfsDelete(sqlite3_vfs *v, const char *n, int s) {
    (void)v; (void)n; (void)s;
    return SQLITE_OK;
}

static int memVfsAccess(sqlite3_vfs *v, const char *zName, int flags, int *pResOut) {
    (void)v; (void)zName; (void)flags;
    *pResOut = 0;  /* file does not exist */
    return SQLITE_OK;
}

static int memVfsFullPathname(sqlite3_vfs *v, const char *zIn, int nOut, char *zOut) {
    (void)v;
    int n = (int)strlen(zIn);
    if (n >= nOut) n = nOut - 1;
    memcpy(zOut, zIn, n);
    zOut[n] = '\0';
    return SQLITE_OK;
}

static int memVfsRandomness(sqlite3_vfs *v, int nByte, char *zOut) {
    (void)v;
    memset(zOut, 0, nByte);
    return nByte;
}

static int memVfsSleep(sqlite3_vfs *v, int microseconds) {
    (void)v; (void)microseconds;
    return 0;
}

static int memVfsCurrentTime(sqlite3_vfs *v, double *pOut) {
    (void)v;
    *pOut = 2460000.5;  /* arbitrary Julian day number */
    return SQLITE_OK;
}

static int memVfsGetLastError(sqlite3_vfs *v, int n, char *buf) {
    (void)v; (void)n; (void)buf;
    return 0;
}

static sqlite3_vfs memVfs = {
    1,                          /* iVersion */
    (int)sizeof(MemFile),       /* szOsFile */
    512,                        /* mxPathname */
    0,                          /* pNext */
    "memvfs",                   /* zName */
    0,                          /* pAppData */
    memVfsOpen,
    memVfsDelete,
    memVfsAccess,
    memVfsFullPathname,
    0, 0, 0, 0,                /* xDlOpen, xDlError, xDlSym, xDlClose */
    memVfsRandomness,
    memVfsSleep,
    memVfsCurrentTime,
    memVfsGetLastError,
};

/* ── OS interface required by SQLITE_OS_OTHER ─────────────────────── */

int sqlite3_os_init(void) {
    return sqlite3_vfs_register(&memVfs, 1 /* make default */);
}

int sqlite3_os_end(void) {
    return SQLITE_OK;
}
