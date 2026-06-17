//! Node-compatibility of the sandbox `fs` object.
//!
//! These lock in the contract that lets promise-based, Node-style filesystem
//! consumers (e.g. `isomorphic-git`) use the sandbox `fs` directly:
//!   * `fs.promises` is an enumerable namespace exposing every method such a
//!     library binds (readFile/writeFile/mkdir/rmdir/unlink/stat/lstat/readdir/
//!     readlink/symlink) — so its `*.bind(...)` wrapping never hits `undefined`;
//!   * `fs.stat`/`fs.lstat` return a `fs.Stats`-like object with `isFile()` /
//!     `isDirectory()` / `isSymbolicLink()` predicate methods;
//!   * `fs.promises.readFile` returns bytes by default (Node semantics);
//!   * rejected operations carry a Node-style `err.code` (e.g. `ENOENT`).
//!
//! Driven through `execute_stateless` against the real filesystem (a temp dir),
//! with assertions read back from captured `console.log` output.

use std::sync::Once;

use server::engine::fs::FsConfig;
use server::engine::opa::{EvalMode, PolicyChain};
use server::engine::{initialize_v8, ExecutionConfig, HardeningConfig};

static INIT: Once = Once::new();

fn ensure_v8() {
    INIT.call_once(initialize_v8);
}

/// An fs config whose policy chain allows every operation (empty chain in
/// `All` mode is vacuously true).
fn allow_all_fs() -> FsConfig {
    FsConfig::new(std::sync::Arc::new(PolicyChain::new(vec![], EvalMode::All)))
}

/// A fresh temp directory that exists on disk; returned as a string path.
fn temp_dir(tag: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!(
        "mcp-fs-node-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn console_tree() -> (sled::Tree, std::path::PathBuf) {
    let tmp = std::env::temp_dir().join(format!(
        "mcp-fs-node-console-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let db = sled::open(&tmp).expect("open sled db");
    let tree = db.open_tree("console").expect("open tree");
    (tree, tmp)
}

fn read_console(tree: &sled::Tree) -> String {
    let mut buf = Vec::new();
    for entry in tree.iter().flatten() {
        buf.extend_from_slice(&entry.1);
    }
    String::from_utf8_lossy(&buf).to_string()
}

/// Run `code` with `fs` enabled (real filesystem) and return captured console.
fn run_fs(code: &str) -> String {
    run_fs_hardened(code, HardeningConfig::default())
}

/// As [`run_fs`], but with an explicit sandbox-hardening configuration.
fn run_fs_hardened(code: &str, hardening: HardeningConfig) -> String {
    let (tree, tmp) = console_tree();
    let fsc = allow_all_fs();
    let config = ExecutionConfig::new(32 * 1024 * 1024)
        .console_tree(tree.clone())
        .maybe_fs_config(Some(&fsc))
        .hardening(hardening);
    let (result, _oom) = server::engine::execute_stateless(code, config);
    assert!(result.is_ok(), "execution failed: {:?}", result);
    let out = read_console(&tree);
    let _ = std::fs::remove_dir_all(&tmp);
    out
}

#[test]
fn fs_promises_namespace_is_present_and_bindable() {
    ensure_v8();
    // Mirrors how isomorphic-git's FileSystem wrapper detects and binds a
    // filesystem: it reads `fs.promises` (must be an enumerable own property)
    // and binds each method. Before the fix, the missing methods made
    // `fs.promises.lstat.bind(...)` throw "Cannot read properties of undefined
    // (reading 'bind')".
    let out = run_fs(
        r#"
        const desc = Object.getOwnPropertyDescriptor(fs, 'promises');
        console.log("enumerable=" + !!(desc && desc.enumerable));
        function wrapLikeIsomorphicGit(fs) {
            const promises = Object.getOwnPropertyDescriptor(fs, 'promises');
            if (!(promises && promises.enumerable)) throw new Error("no enumerable promises");
            const o = {};
            o._readFile = fs.promises.readFile.bind(fs.promises);
            o._writeFile = fs.promises.writeFile.bind(fs.promises);
            o._mkdir = fs.promises.mkdir.bind(fs.promises);
            o._rmdir = fs.promises.rmdir.bind(fs.promises);
            o._unlink = fs.promises.unlink.bind(fs.promises);
            o._stat = fs.promises.stat.bind(fs.promises);
            o._lstat = fs.promises.lstat.bind(fs.promises);
            o._readdir = fs.promises.readdir.bind(fs.promises);
            o._readlink = fs.promises.readlink.bind(fs.promises);
            o._symlink = fs.promises.symlink.bind(fs.promises);
            return o;
        }
        try {
            wrapLikeIsomorphicGit(fs);
            console.log("wrap=ok");
        } catch (e) {
            console.log("wrap=err:" + e.message);
        }
        "#,
    );
    assert!(out.contains("enumerable=true"), "fs.promises must be enumerable, got: {out}");
    assert!(
        out.contains("wrap=ok"),
        "isomorphic-git-style binding must not throw, got: {out}"
    );
}

#[test]
fn fs_promises_roundtrip_stats_and_error_codes() {
    ensure_v8();
    let dir = temp_dir("rt");
    let dir_s = dir.to_str().unwrap();
    let code = format!(
        r#"(async () => {{
            const dir = {dir:?};
            await fs.promises.mkdir(dir + "/sub", {{ recursive: true }});
            await fs.promises.writeFile(dir + "/sub/a.txt", "hello");

            // Node semantics: default read returns bytes, encoding returns text.
            const buf = await fs.promises.readFile(dir + "/sub/a.txt");
            console.log("isU8=" + (buf instanceof Uint8Array));
            console.log("bytes=" + String.fromCharCode.apply(null, Array.from(buf)));
            const txt = await fs.promises.readFile(dir + "/sub/a.txt", "utf8");
            console.log("isStr=" + (typeof txt === "string") + ",txt=" + txt);

            // Stats object with predicate methods.
            const fst = await fs.promises.stat(dir + "/sub/a.txt");
            console.log("file_isFile=" + fst.isFile() + ",file_isDir=" + fst.isDirectory()
                + ",file_isSym=" + fst.isSymbolicLink() + ",size=" + fst.size);
            const dst = await fs.promises.lstat(dir + "/sub");
            console.log("dir_isDir=" + dst.isDirectory() + ",dir_isFile=" + dst.isFile());

            // Missing path rejects with a Node-style code.
            try {{
                await fs.promises.readFile(dir + "/nope.txt");
                console.log("missing=no-error");
            }} catch (e) {{
                console.log("missing_code=" + e.code);
            }}

            // mkdir of an existing dir rejects with EEXIST (non-recursive).
            try {{
                await fs.promises.mkdir(dir + "/sub");
                console.log("mkdir=no-error");
            }} catch (e) {{
                console.log("mkdir_code=" + e.code);
            }}
        }})()"#,
        dir = dir_s,
    );
    let out = run_fs(&code);
    assert!(out.contains("isU8=true"), "default promises.readFile must return Uint8Array, got: {out}");
    assert!(out.contains("bytes=hello"), "byte content mismatch, got: {out}");
    assert!(out.contains("isStr=true,txt=hello"), "utf8 read must return string, got: {out}");
    assert!(
        out.contains("file_isFile=true,file_isDir=false,file_isSym=false,size=5"),
        "Stats predicates wrong for file, got: {out}"
    );
    assert!(out.contains("dir_isDir=true,dir_isFile=false"), "Stats predicates wrong for dir, got: {out}");
    assert!(out.contains("missing_code=ENOENT"), "missing file must surface ENOENT, got: {out}");
    assert!(out.contains("mkdir_code=EEXIST"), "mkdir of existing dir must surface EEXIST, got: {out}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn fs_symlink_lstat_readlink_roundtrip() {
    ensure_v8();
    let dir = temp_dir("sym");
    let dir_s = dir.to_str().unwrap();
    let code = format!(
        r#"(async () => {{
            const dir = {dir:?};
            await fs.promises.writeFile(dir + "/target.txt", "T");
            // Node signature: symlink(target, path).
            await fs.promises.symlink(dir + "/target.txt", dir + "/link.txt");

            const ls = await fs.promises.lstat(dir + "/link.txt");
            console.log("link_isSym=" + ls.isSymbolicLink() + ",link_isFile=" + ls.isFile());

            // stat() follows the link to the regular file.
            const st = await fs.promises.stat(dir + "/link.txt");
            console.log("target_isFile=" + st.isFile() + ",target_isSym=" + st.isSymbolicLink());

            const tgt = await fs.promises.readlink(dir + "/link.txt");
            console.log("readlink_ok=" + (tgt === dir + "/target.txt"));
        }})()"#,
        dir = dir_s,
    );
    let out = run_fs(&code);
    assert!(out.contains("link_isSym=true,link_isFile=false"), "lstat must report a symlink, got: {out}");
    assert!(out.contains("target_isFile=true,target_isSym=false"), "stat must follow the link, got: {out}");
    assert!(out.contains("readlink_ok=true"), "readlink must return the target, got: {out}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn fs_node_compat_survives_full_hardening() {
    ensure_v8();
    // The fs wrapper dispatches through a captured `Deno.core.ops` reference;
    // full hardening freezes that object. A frozen op table stays callable, so
    // the promises surface and error-code tagging must keep working.
    let dir = temp_dir("hard");
    let dir_s = dir.to_str().unwrap();
    let code = format!(
        r#"(async () => {{
            const dir = {dir:?};
            const d = Object.getOwnPropertyDescriptor(fs, "promises");
            console.log("frozen_ops=" + Object.isFrozen(Deno.core.ops));
            console.log("enumerable=" + !!(d && d.enumerable));
            await fs.promises.writeFile(dir + "/h.txt", "hi");
            const buf = await fs.promises.readFile(dir + "/h.txt");
            console.log("len=" + buf.length);
            try {{
                await fs.promises.readFile(dir + "/absent");
                console.log("missing=no-error");
            }} catch (e) {{
                console.log("missing_code=" + e.code);
            }}
        }})()"#,
        dir = dir_s,
    );
    let out = run_fs_hardened(&code, HardeningConfig::all());
    assert!(out.contains("frozen_ops=true"), "ops should be frozen under full hardening, got: {out}");
    assert!(out.contains("enumerable=true"), "fs.promises must stay enumerable, got: {out}");
    assert!(out.contains("len=2"), "promises.readFile must work under hardening, got: {out}");
    assert!(out.contains("missing_code=ENOENT"), "error codes must work under hardening, got: {out}");
    let _ = std::fs::remove_dir_all(&dir);
}
