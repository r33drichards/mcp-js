//! The CommonJS `require()` shim.
//!
//! Locks in the contract that Node-flavored code agents habitually write can
//! resolve the supported built-ins — and that everything else fails with an
//! actionable message instead of `ReferenceError: require is not defined`:
//!   * `require` exists unconditionally (no policies needed);
//!   * `require('fs')` / `require('node:fs')` return the sandbox `fs` global,
//!     `require('fs/promises')` returns `fs.promises` — only when the server
//!     has a filesystem policy, otherwise the error explains how to enable it;
//!   * `require('path')` is a POSIX `path` implementation with Node semantics;
//!   * unknown specifiers throw `MODULE_NOT_FOUND` naming the supported set.
//!
//! Driven through `execute_stateless`, with assertions read back from
//! captured `console.log` output (same harness as fs_node_compat.rs).

use std::sync::Once;

use server::engine::fs::FsConfig;
use server::engine::opa::{EvalMode, PolicyChain};
use server::engine::{initialize_v8, ExecutionConfig};

static INIT: Once = Once::new();

fn ensure_v8() {
    INIT.call_once(initialize_v8);
}

/// An fs config whose policy chain allows every operation (empty chain in
/// `All` mode is vacuously true).
fn allow_all_fs() -> FsConfig {
    FsConfig::new(std::sync::Arc::new(PolicyChain::new(vec![], EvalMode::All)))
}

fn console_tree() -> (sled::Tree, std::path::PathBuf) {
    let tmp = std::env::temp_dir().join(format!(
        "mcp-require-console-{}-{}",
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

/// Run `code` WITHOUT any fs policy and return captured console output.
fn run_plain(code: &str) -> String {
    let (tree, tmp) = console_tree();
    let config = ExecutionConfig::new(32 * 1024 * 1024).console_tree(tree.clone());
    let (result, _oom) = server::engine::execute_stateless(code, config);
    assert!(result.is_ok(), "execution failed: {:?}", result);
    let out = read_console(&tree);
    let _ = std::fs::remove_dir_all(&tmp);
    out
}

/// Run `code` with `fs` enabled (real filesystem, allow-all policy).
fn run_fs(code: &str) -> String {
    let (tree, tmp) = console_tree();
    let fsc = allow_all_fs();
    let config = ExecutionConfig::new(32 * 1024 * 1024)
        .console_tree(tree.clone())
        .maybe_fs_config(Some(&fsc));
    let (result, _oom) = server::engine::execute_stateless(code, config);
    assert!(result.is_ok(), "execution failed: {:?}", result);
    let out = read_console(&tree);
    let _ = std::fs::remove_dir_all(&tmp);
    out
}

/// A fresh temp directory that exists on disk; returned as a string path.
fn temp_dir(tag: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!(
        "mcp-require-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&p).unwrap();
    p
}

#[test]
fn require_exists_without_any_policy() {
    ensure_v8();
    let out = run_plain(
        r#"
        console.log("type=" + typeof require);
        console.log("resolve=" + typeof require.resolve);
        "#,
    );
    assert!(out.contains("type=function"), "require must be a function, got: {out}");
    assert!(out.contains("resolve=function"), "require.resolve must exist, got: {out}");
}

#[test]
fn unknown_module_throws_module_not_found_naming_builtins() {
    ensure_v8();
    let out = run_plain(
        r#"
        try {
            require('left-pad');
        } catch (e) {
            console.log("code=" + e.code);
            console.log("msg=" + e.message);
        }
        "#,
    );
    assert!(out.contains("code=MODULE_NOT_FOUND"), "got: {out}");
    assert!(out.contains("Cannot find module 'left-pad'"), "got: {out}");
    assert!(out.contains("fs, fs/promises, path"), "message must name the supported built-ins, got: {out}");
    assert!(out.contains("npm:"), "message must point npm users at ES imports, got: {out}");
}

#[test]
fn relative_specifier_gets_file_module_hint() {
    ensure_v8();
    let out = run_plain(
        r#"
        try {
            require('./skill.js');
        } catch (e) {
            console.log("msg=" + e.message);
        }
        "#,
    );
    assert!(out.contains("Cannot find module './skill.js'"), "got: {out}");
    assert!(out.contains("ES module"), "got: {out}");
}

#[test]
fn require_fs_without_policy_explains_how_to_enable() {
    ensure_v8();
    let out = run_plain(
        r#"
        try {
            require('fs');
        } catch (e) {
            console.log("msg=" + e.message);
        }
        "#,
    );
    assert!(out.contains("filesystem access is disabled"), "got: {out}");
    assert!(out.contains("--policies-json"), "error must say how to enable fs, got: {out}");
}

#[test]
fn require_fs_returns_the_sandbox_fs() {
    ensure_v8();
    let out = run_fs(
        r#"
        console.log("fs=" + (require('fs') === globalThis.fs));
        console.log("nodefs=" + (require('node:fs') === globalThis.fs));
        console.log("promises=" + (require('fs/promises') === globalThis.fs.promises));
        console.log("nodepromises=" + (require('node:fs/promises') === globalThis.fs.promises));
        "#,
    );
    assert!(out.contains("fs=true"), "got: {out}");
    assert!(out.contains("nodefs=true"), "got: {out}");
    assert!(out.contains("promises=true"), "got: {out}");
    assert!(out.contains("nodepromises=true"), "got: {out}");
}

#[test]
fn required_fs_round_trips_a_file() {
    ensure_v8();
    let dir = temp_dir("rt");
    let dir_s = dir.to_str().unwrap();
    let code = format!(
        r#"
        const fsm = require('fs/promises');
        const path = require('path');
        const file = path.join({dir_s:?}, "hello.txt");
        await fsm.writeFile(file, "hi from require");
        console.log("read=" + await fsm.readFile(file, "utf8"));
        "#
    );
    let out = run_fs(&code);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(out.contains("read=hi from require"), "got: {out}");
}

#[test]
fn path_module_matches_node_posix_semantics() {
    ensure_v8();
    let out = run_plain(
        r#"
        const path = require('path');
        const checks = [
            ["sep", path.sep, "/"],
            ["join", path.join("a", "b", "..", "c"), "a/c"],
            ["join-abs", path.join("/a", "b"), "/a/b"],
            ["join-empty", path.join(), "."],
            ["normalize", path.normalize("/a//b/../c/"), "/a/c/"],
            ["normalize-dot", path.normalize("a/.."), "."],
            ["normalize-root", path.normalize("/.."), "/"],
            ["resolve", path.resolve("/foo/bar", "./baz"), "/foo/bar/baz"],
            ["resolve-abs", path.resolve("/foo/bar", "/tmp/file"), "/tmp/file"],
            ["resolve-rel", path.resolve("a"), "/a"],
            ["resolve-empty", path.resolve(), "/"],
            ["dirname", path.dirname("/a/b/c"), "/a/b"],
            ["dirname-root", path.dirname("/"), "/"],
            ["dirname-bare", path.dirname("a"), "."],
            ["basename", path.basename("/a/b.txt"), "b.txt"],
            ["basename-ext", path.basename("/a/b.txt", ".txt"), "b"],
            ["basename-same", path.basename(".txt", ".txt"), ""],
            ["extname", path.extname("index.html"), ".html"],
            ["extname-dotfile", path.extname(".bashrc"), ""],
            ["extname-trailingdot", path.extname("file."), "."],
            ["extname-dotdot", path.extname(".."), ""],
            ["relative", path.relative("/a/b", "/a/c/d"), "../c/d"],
            ["isabs-yes", path.isAbsolute("/x"), true],
            ["isabs-no", path.isAbsolute("x"), false],
            ["posix", path.posix === path, true],
        ];
        for (const [name, got, want] of checks) {
            console.log(name + "=" + (got === want ? "ok" : "FAIL(got " + JSON.stringify(got) + ")"));
        }
        const p = path.parse("/home/user/file.txt");
        console.log("parse=" + JSON.stringify(p));
        console.log("format=" + path.format(p));
        console.log("parse-bare-dir=" + JSON.stringify(path.parse("file.txt").dir));
        "#,
    );
    for line in out.lines() {
        assert!(!line.contains("FAIL"), "path check failed: {line}\nfull output: {out}");
    }
    assert!(
        out.contains(r#"parse={"root":"/","dir":"/home/user","base":"file.txt","ext":".txt","name":"file"}"#),
        "got: {out}"
    );
    assert!(out.contains("format=/home/user/file.txt"), "got: {out}");
    assert!(out.contains(r#"parse-bare-dir="""#), "got: {out}");
}

#[test]
fn require_resolve_works_without_capabilities() {
    ensure_v8();
    // Resolvability is independent of whether fs is enabled: 'fs' is a known
    // module name even on a server with no filesystem policy.
    let out = run_plain(
        r#"
        console.log("fs=" + require.resolve('fs'));
        console.log("nodepath=" + require.resolve('node:path'));
        try {
            require.resolve('express');
        } catch (e) {
            console.log("code=" + e.code);
        }
        "#,
    );
    assert!(out.contains("fs=fs"), "got: {out}");
    assert!(out.contains("nodepath=node:path"), "got: {out}");
    assert!(out.contains("code=MODULE_NOT_FOUND"), "got: {out}");
}
