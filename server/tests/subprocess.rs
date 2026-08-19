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
