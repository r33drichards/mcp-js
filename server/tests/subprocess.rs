use std::sync::{Arc, Once};

use server::engine::{
    ExecutionConfig, execute_stateless, initialize_v8,
    opa::{EvalMode, PolicyChain},
    subprocess::SubprocessConfig,
};

static INIT: Once = Once::new();

#[test]
fn subprocess_runs_without_fetch_or_filesystem_config() {
    INIT.call_once(initialize_v8);
    let policy = Arc::new(PolicyChain::new(vec![], EvalMode::All));
    let subprocess = SubprocessConfig::new(policy);
    let code = r#"
        const output = await new Deno.Command('printf', { args: ['hello'] }).output();
        if (output.code !== 0 || new TextDecoder().decode(output.stdout) !== 'hello') {
            throw new Error('unexpected subprocess output');
        }
    "#;
    let (result, _) = execute_stateless(
        code,
        ExecutionConfig::new(64 * 1024 * 1024)
            .maybe_subprocess_config(Some(&subprocess)),
    );
    assert!(result.is_ok(), "subprocess execution failed: {result:?}");
}


#[test]
fn child_process_fork_relays_node_ipc() {
    INIT.call_once(initialize_v8);
    let policy = Arc::new(PolicyChain::new(vec![], EvalMode::All));
    let subprocess = SubprocessConfig::new(policy);
    let child = std::env::temp_dir().join(format!(
        "mcp-node-fork-child-{}-{}.mjs",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    std::fs::write(
        &child,
        "process.on('message', (message) => { if (process.argv[2] !== 'silent') process.send(message); });\n",
    )
    .unwrap();
    let child_path = serde_json::to_string(child.to_str().unwrap()).unwrap();
    let victim = child.with_extension("victim");
    std::fs::write(&victim, "keep").unwrap();
    let victim_path = serde_json::to_string(victim.to_str().unwrap()).unwrap();
    let code = format!(
        r#"
        globalThis.__NODE_TEST_EXEC_PATH__ = 'node';
        const {{ fork }} = await import('node:child_process');
        const {{ once }} = await import('node:events');
        const child = fork({child_path});
        child.send({{ hello: 'world' }});
        const [message] = await once(child, 'message');
        if (message?.hello !== 'world') throw new Error('fork IPC did not round-trip');
        child.disconnect();
        await once(child, 'exit');

        const silent = fork({child_path}, ['silent']);
        silent._controlPath = {victim_path};
        silent.send('no reply');
        silent.disconnect();
        await once(silent, 'exit');
        "#,
    );
    let (result, _) = execute_stateless(
        &code,
        ExecutionConfig::new(64 * 1024 * 1024)
            .maybe_subprocess_config(Some(&subprocess)),
    );
    let victim_contents = std::fs::read_to_string(&victim).unwrap();
    let _ = std::fs::remove_file(child);
    let _ = std::fs::remove_file(victim);
    assert!(result.is_ok(), "child_process fork failed: {result:?}");
    assert_eq!(victim_contents, "keep", "fork control path escaped private state");
}
