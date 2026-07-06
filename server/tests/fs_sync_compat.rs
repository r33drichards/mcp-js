//! Node-style synchronous fs API (`fs.*Sync`).
//!
//! Locks in the contract for the sync twins of the fs ops:
//!   * `readFileSync`/`writeFileSync`/`existsSync`/… work at top level and in
//!     event-loop continuations (the isolate thread cannot block on its own
//!     runtime — these ops bridge to a helper thread, see `fs::run_sync`);
//!   * sync ops share the async ops' policy gate under the SAME operation
//!     names, so existing Rego policies apply unchanged;
//!   * `existsSync` follows Node and never throws — policy denials and IO
//!     errors read as `false`;
//!   * errors carry Node-style `err.code` (e.g. `ENOENT`);
//!   * everything works against the virtual overlay mount too, where the
//!     implementation must run on a current-thread runtime (deno_unsync).
//!
//! Driven through `execute_stateless`, with assertions read back from
//! captured `console.log` output (same harness as fs_node_compat.rs).

use std::sync::{Arc, Once};

use server::engine::fs::{FsConfig, FsMountHandle};
use server::engine::fs_mount::SessionMount;
use server::engine::fs_store::FsStore;
use server::engine::opa::{
    build_policy_chain, EvalMode, OperationPolicies, PolicyChain, PolicySource,
};
use server::engine::{initialize_v8, ExecutionConfig};

static INIT: Once = Once::new();

fn ensure_v8() {
    INIT.call_once(initialize_v8);
}

/// An fs config whose policy chain allows every operation (empty chain in
/// `All` mode is vacuously true).
fn allow_all_fs() -> FsConfig {
    FsConfig::new(Arc::new(PolicyChain::new(vec![], EvalMode::All)))
}

fn console_tree() -> (sled::Tree, std::path::PathBuf) {
    let tmp = std::env::temp_dir().join(format!(
        "mcp-fs-sync-console-{}-{}",
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

/// A fresh temp directory that exists on disk; returned as a string path.
fn temp_dir(tag: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!(
        "mcp-fs-sync-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// Run `code` with the given fs config (and optional overlay mount); return
/// captured console output.
fn run_with(code: &str, fsc: &FsConfig, mount: Option<FsMountHandle>) -> String {
    let (tree, tmp) = console_tree();
    let config = ExecutionConfig::new(32 * 1024 * 1024)
        .console_tree(tree.clone())
        .maybe_fs_config(Some(fsc))
        .maybe_fs_mount(mount);
    let (result, _oom) = server::engine::execute_stateless(code, config);
    assert!(result.is_ok(), "execution failed: {:?}", result);
    let out = read_console(&tree);
    let _ = std::fs::remove_dir_all(&tmp);
    out
}

/// Run `code` with `fs` enabled against the real filesystem (allow-all policy).
fn run_fs(code: &str) -> String {
    run_with(code, &allow_all_fs(), None)
}

#[test]
fn require_fs_snippet_with_sync_calls_works() {
    ensure_v8();
    // The motivating case: an agent pastes Node-flavored code using require()
    // plus the sync fs API. This exact shape previously failed with
    // "ReferenceError: require is not defined".
    let dir = temp_dir("snippet");
    std::fs::write(dir.join("SKILL.md"), "# craftos-sim skill").unwrap();
    let dir_s = dir.to_str().unwrap();
    let code = format!(
        r#"
        const fs = require('fs');
        const skillPath = {dir_s:?} + '/SKILL.md';
        if (fs.existsSync(skillPath)) {{
            const skillContent = fs.readFileSync(skillPath, 'utf8');
            console.log(skillContent);
        }} else {{
            console.log("skill not found");
        }}
        "#
    );
    let out = run_fs(&code);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(out.contains("# craftos-sim skill"), "got: {out}");
}

#[test]
fn sync_roundtrip_on_real_fs() {
    ensure_v8();
    let dir = temp_dir("rt");
    let dir_s = dir.to_str().unwrap();
    let code = format!(
        r#"
        const d = {dir_s:?};
        fs.writeFileSync(d + "/a.txt", "hello");
        fs.appendFileSync(d + "/a.txt", " world");
        console.log("text=" + fs.readFileSync(d + "/a.txt", "utf8"));
        const bytes = fs.readFileSync(d + "/a.txt");
        console.log("bytes=" + (bytes instanceof Uint8Array) + ":" + bytes.length);

        fs.writeFileSync(d + "/bin.dat", new Uint8Array([1, 2, 3]));
        console.log("bin=" + Array.from(fs.readFileSync(d + "/bin.dat")).join(","));

        fs.mkdirSync(d + "/sub/deep", {{ recursive: true }});
        console.log("dir=" + fs.statSync(d + "/sub/deep").isDirectory());
        console.log("file=" + fs.statSync(d + "/a.txt").isFile());
        console.log("size=" + fs.statSync(d + "/a.txt").size);

        fs.copyFileSync(d + "/a.txt", d + "/b.txt");
        fs.renameSync(d + "/b.txt", d + "/c.txt");
        console.log("entries=" + fs.readdirSync(d).sort().join(","));

        fs.unlinkSync(d + "/c.txt");
        console.log("gone=" + !fs.existsSync(d + "/c.txt"));
        fs.rmSync(d + "/sub", {{ recursive: true }});
        console.log("subgone=" + !fs.existsSync(d + "/sub"));
        "#
    );
    let out = run_fs(&code);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(out.contains("text=hello world"), "got: {out}");
    assert!(out.contains("bytes=true:11"), "got: {out}");
    assert!(out.contains("bin=1,2,3"), "got: {out}");
    assert!(out.contains("dir=true"), "got: {out}");
    assert!(out.contains("file=true"), "got: {out}");
    assert!(out.contains("size=11"), "got: {out}");
    assert!(out.contains("entries=a.txt,bin.dat,c.txt,sub"), "got: {out}");
    assert!(out.contains("gone=true"), "got: {out}");
    assert!(out.contains("subgone=true"), "got: {out}");
}

#[test]
fn symlink_sync_roundtrip() {
    ensure_v8();
    let dir = temp_dir("ln");
    let dir_s = dir.to_str().unwrap();
    let code = format!(
        r#"
        const d = {dir_s:?};
        fs.writeFileSync(d + "/target.txt", "data");
        fs.symlinkSync(d + "/target.txt", d + "/link");
        console.log("readlink=" + (fs.readlinkSync(d + "/link") === d + "/target.txt"));
        console.log("lstat=" + fs.lstatSync(d + "/link").isSymbolicLink());
        console.log("follow=" + fs.readFileSync(d + "/link", "utf8"));
        "#
    );
    let out = run_fs(&code);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(out.contains("readlink=true"), "got: {out}");
    assert!(out.contains("lstat=true"), "got: {out}");
    assert!(out.contains("follow=data"), "got: {out}");
}

#[test]
fn readfilesync_missing_file_carries_enoent_code() {
    ensure_v8();
    let dir = temp_dir("enoent");
    let dir_s = dir.to_str().unwrap();
    let code = format!(
        r#"
        try {{
            fs.readFileSync({dir_s:?} + "/nope.txt", "utf8");
            console.log("err=none");
        }} catch (e) {{
            console.log("err=" + e.code);
        }}
        "#
    );
    let out = run_fs(&code);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(out.contains("err=ENOENT"), "got: {out}");
}

#[test]
fn existssync_never_throws() {
    ensure_v8();
    let out = run_fs(
        r#"
        console.log("missing=" + fs.existsSync("/definitely/not/a/real/path/xyz"));
        console.log("nonstring=" + fs.existsSync(42));
        "#,
    );
    assert!(out.contains("missing=false"), "got: {out}");
    assert!(out.contains("nonstring=false"), "got: {out}");
}

#[test]
fn sync_ops_work_in_event_loop_continuations() {
    ensure_v8();
    // Sync ops must work not just in top-level module code but after awaits,
    // i.e. when invoked from a continuation driven by the isolate's event
    // loop — the bridge thread pattern must hold there too.
    let dir = temp_dir("loop");
    let dir_s = dir.to_str().unwrap();
    let code = format!(
        r#"
        const d = {dir_s:?};
        await new Promise((resolve) => setTimeout(resolve, 5));
        fs.writeFileSync(d + "/late.txt", "written after await");
        console.log("late=" + fs.readFileSync(d + "/late.txt", "utf8"));
        "#
    );
    let out = run_fs(&code);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(out.contains("late=written after await"), "got: {out}");
}

#[test]
fn sync_and_async_interleave_without_deadlock() {
    ensure_v8();
    let dir = temp_dir("mix");
    let dir_s = dir.to_str().unwrap();
    let code = format!(
        r#"
        const d = {dir_s:?};
        const pending = fs.writeFile(d + "/async.txt", "from async");
        fs.writeFileSync(d + "/sync.txt", "from sync");
        await pending;
        console.log("async=" + fs.readFileSync(d + "/async.txt", "utf8"));
        console.log("sync=" + await fs.readFile(d + "/sync.txt", "utf8"));
        "#
    );
    let out = run_fs(&code);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(out.contains("async=from async"), "got: {out}");
    assert!(out.contains("sync=from sync"), "got: {out}");
}

// ── Policy gating ────────────────────────────────────────────────────────

fn write_rego(dir: &std::path::Path, content: &str) -> std::path::PathBuf {
    let p = dir.join("fs.rego");
    std::fs::write(&p, content).unwrap();
    p
}

/// A policy chain from a local Rego file gating `data.mcp.filesystem.allow`.
fn fs_config_with_rego(rego_path: &std::path::Path) -> FsConfig {
    let op = OperationPolicies {
        mode: EvalMode::All,
        policies: vec![PolicySource {
            url: format!("file://{}", rego_path.display()),
            policy_path: None,
            rule: None,
        }],
    };
    let chain =
        build_policy_chain(&op, "mcp/filesystem", "data.mcp.filesystem.allow").unwrap();
    FsConfig::new(Arc::new(chain))
}

#[test]
fn sync_ops_share_the_async_policy_operation_names() {
    ensure_v8();
    let dir = temp_dir("pol");
    let dir_s = dir.to_str().unwrap();
    std::fs::write(dir.join("ok.txt"), "readable").unwrap();
    // Reads allowed under the sandbox dir; writes denied everywhere. The
    // operation names are the async ones ("readFile", "writeFile") — sync
    // variants must be gated identically.
    let rego = write_rego(
        &dir,
        &format!(
            r#"
package mcp.filesystem

default allow = false

allow if {{
    input.operation in {{"readFile", "exists"}}
    startswith(input.path, {dir_s:?})
}}
"#
        ),
    );
    let fsc = fs_config_with_rego(&rego);
    let code = format!(
        r#"
        const d = {dir_s:?};
        console.log("read=" + fs.readFileSync(d + "/ok.txt", "utf8"));
        console.log("outside=" + fs.existsSync("/etc/passwd"));
        try {{
            fs.writeFileSync(d + "/no.txt", "x");
            console.log("write=allowed");
        }} catch (e) {{
            console.log("write=" + (e.message.includes("denied by policy") ? "denied" : e.message));
        }}
        try {{
            fs.readFileSync("/etc/hostname", "utf8");
            console.log("readout=allowed");
        }} catch (e) {{
            console.log("readout=" + (e.message.includes("denied by policy") ? "denied" : e.message));
        }}
        "#
    );
    let out = run_with(&code, &fsc, None);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(out.contains("read=readable"), "got: {out}");
    // existsSync swallows the policy denial per Node's never-throw contract.
    assert!(out.contains("outside=false"), "got: {out}");
    assert!(out.contains("write=denied"), "got: {out}");
    assert!(out.contains("readout=denied"), "got: {out}");
}

// ── Overlay mount ────────────────────────────────────────────────────────

#[test]
fn sync_ops_operate_on_the_overlay_mount() {
    ensure_v8();
    // With a mount attached, sync ops must route to the virtual overlay (via
    // the bridge thread's current-thread runtime — deno_unsync asserts that
    // flavor), not the host filesystem.
    let real = temp_dir("mnt-real");
    std::fs::write(real.join("host.txt"), "on host").unwrap();
    let real_s = real.to_str().unwrap();

    let store = FsStore::in_memory();
    let mount = FsMountHandle::new(SessionMount::empty(store));

    let code = format!(
        r#"
        fs.writeFileSync("/work/hello.txt", "overlay data");
        console.log("read=" + fs.readFileSync("/work/hello.txt", "utf8"));
        console.log("exists=" + fs.existsSync("/work/hello.txt"));
        console.log("entries=" + fs.readdirSync("/work").join(","));
        console.log("stat=" + fs.statSync("/work/hello.txt").isFile());
        // The overlay is the whole fs view (passthrough off): host files are
        // invisible even though they exist on disk.
        console.log("host=" + fs.existsSync({real_s:?} + "/host.txt"));
        fs.mkdirSync("/work/sub", {{ recursive: true }});
        fs.renameSync("/work/hello.txt", "/work/sub/moved.txt");
        console.log("moved=" + fs.readFileSync("/work/sub/moved.txt", "utf8"));
        fs.rmSync("/work/sub", {{ recursive: true }});
        console.log("gone=" + !fs.existsSync("/work/sub/moved.txt"));
        "#
    );
    let out = run_with(&code, &allow_all_fs(), Some(mount));
    let _ = std::fs::remove_dir_all(&real);
    assert!(out.contains("read=overlay data"), "got: {out}");
    assert!(out.contains("exists=true"), "got: {out}");
    assert!(out.contains("entries=hello.txt"), "got: {out}");
    assert!(out.contains("stat=true"), "got: {out}");
    assert!(out.contains("host=false"), "got: {out}");
    assert!(out.contains("moved=overlay data"), "got: {out}");
    assert!(out.contains("gone=true"), "got: {out}");
}
