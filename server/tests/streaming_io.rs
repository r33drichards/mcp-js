/// Tests for stdin/stdout/stderr streaming I/O between isolate executions.
///
/// These tests verify:
/// - `__stdin__` global injection via the `stdin` parameter
/// - `console.log`/`console.info` capture into stdout
/// - `console.error`/`console.warn` capture into stderr
/// - Composing executions by piping output → stdin

use std::sync::{Arc, Mutex, Once};
use server::engine::{Engine, initialize_v8, execute_stateless, execute_stateful, PipelineStage};

static INIT: Once = Once::new();

fn ensure_v8() {
    INIT.call_once(|| {
        initialize_v8();
    });
}

fn no_handle() -> Arc<Mutex<Option<v8::IsolateHandle>>> {
    Arc::new(Mutex::new(None))
}

const HEAP: usize = 8 * 1024 * 1024;

// ── __stdin__ injection tests ───────────────────────────────────────────

#[test]
fn test_stdin_available_as_global() {
    ensure_v8();
    let (result, _) = execute_stateless("__stdin__", HEAP, no_handle(), &[], HEAP, Some("hello world"));
    assert!(result.is_ok(), "Should succeed, got: {:?}", result);
    assert_eq!(result.unwrap().result, "hello world");
}

#[test]
fn test_stdin_undefined_when_not_provided() {
    ensure_v8();
    let (result, _) = execute_stateless("typeof __stdin__", HEAP, no_handle(), &[], HEAP, None);
    assert!(result.is_ok(), "Should succeed, got: {:?}", result);
    assert_eq!(result.unwrap().result, "undefined");
}

#[test]
fn test_stdin_json_round_trip() {
    ensure_v8();
    let json_str = r#"{"name":"Alice","score":42}"#;
    let code = r#"var data = JSON.parse(__stdin__); data.score * 2;"#;
    let (result, _) = execute_stateless(code, HEAP, no_handle(), &[], HEAP, Some(json_str));
    assert!(result.is_ok(), "Should succeed, got: {:?}", result);
    assert_eq!(result.unwrap().result, "84");
}

#[test]
fn test_stdin_empty_string() {
    ensure_v8();
    let (result, _) = execute_stateless("__stdin__.length", HEAP, no_handle(), &[], HEAP, Some(""));
    assert!(result.is_ok(), "Should succeed, got: {:?}", result);
    assert_eq!(result.unwrap().result, "0");
}

#[test]
fn test_stdin_special_characters() {
    ensure_v8();
    let input = "line1\nline2\ttab\"quote";
    let (result, _) = execute_stateless("__stdin__", HEAP, no_handle(), &[], HEAP, Some(input));
    assert!(result.is_ok(), "Should succeed, got: {:?}", result);
    assert_eq!(result.unwrap().result, input);
}

#[test]
fn test_stdin_stateful_mode() {
    ensure_v8();
    let (result, _) = execute_stateful(
        "__stdin__", None, HEAP, no_handle(), &[], HEAP, Some("stateful input"),
    );
    assert!(result.is_ok(), "Should succeed, got: {:?}", result);
    let (exec, snapshot, _hash) = result.unwrap();
    assert_eq!(exec.result, "stateful input");
    assert!(!snapshot.is_empty());
}

// ── console.log / console.info → stdout ─────────────────────────────────

#[test]
fn test_console_log_captured() {
    ensure_v8();
    let code = r#"console.log("hello"); console.log("world"); "done";"#;
    let (result, _) = execute_stateless(code, HEAP, no_handle(), &[], HEAP, None);
    assert!(result.is_ok(), "Should succeed, got: {:?}", result);
    let exec = result.unwrap();
    assert_eq!(exec.result, "done");
    assert_eq!(exec.stdout, vec!["hello", "world"]);
    assert!(exec.stderr.is_empty());
}

#[test]
fn test_console_info_goes_to_stdout() {
    ensure_v8();
    let code = r#"console.info("info line"); "ok";"#;
    let (result, _) = execute_stateless(code, HEAP, no_handle(), &[], HEAP, None);
    assert!(result.is_ok());
    let exec = result.unwrap();
    assert_eq!(exec.stdout, vec!["info line"]);
}

#[test]
fn test_console_log_multiple_args() {
    ensure_v8();
    let code = r#"console.log("a", 1, true, null); "ok";"#;
    let (result, _) = execute_stateless(code, HEAP, no_handle(), &[], HEAP, None);
    assert!(result.is_ok());
    let exec = result.unwrap();
    assert_eq!(exec.stdout, vec!["a 1 true null"]);
}

// ── console.error / console.warn → stderr ───────────────────────────────

#[test]
fn test_console_error_captured() {
    ensure_v8();
    let code = r#"console.error("oops"); "done";"#;
    let (result, _) = execute_stateless(code, HEAP, no_handle(), &[], HEAP, None);
    assert!(result.is_ok());
    let exec = result.unwrap();
    assert_eq!(exec.result, "done");
    assert_eq!(exec.stderr, vec!["oops"]);
    assert!(exec.stdout.is_empty());
}

#[test]
fn test_console_warn_goes_to_stderr() {
    ensure_v8();
    let code = r#"console.warn("warning!"); "ok";"#;
    let (result, _) = execute_stateless(code, HEAP, no_handle(), &[], HEAP, None);
    assert!(result.is_ok());
    let exec = result.unwrap();
    assert_eq!(exec.stderr, vec!["warning!"]);
}

#[test]
fn test_stdout_and_stderr_separate() {
    ensure_v8();
    let code = r#"
        console.log("out1");
        console.error("err1");
        console.log("out2");
        console.warn("err2");
        "done";
    "#;
    let (result, _) = execute_stateless(code, HEAP, no_handle(), &[], HEAP, None);
    assert!(result.is_ok());
    let exec = result.unwrap();
    assert_eq!(exec.stdout, vec!["out1", "out2"]);
    assert_eq!(exec.stderr, vec!["err1", "err2"]);
}

// ── Composing executions (piping) ───────────────────────────────────────

#[test]
fn test_pipe_output_to_stdin_stateless() {
    ensure_v8();

    // Step 1: produce output
    let (r1, _) = execute_stateless(
        r#"JSON.stringify({ items: [1, 2, 3] })"#,
        HEAP, no_handle(), &[], HEAP, None,
    );
    let exec1 = r1.unwrap();
    assert_eq!(exec1.result, r#"{"items":[1,2,3]}"#);

    // Step 2: consume output as stdin
    let (r2, _) = execute_stateless(
        r#"var data = JSON.parse(__stdin__); data.items.reduce(function(a, b) { return a + b; }, 0);"#,
        HEAP, no_handle(), &[], HEAP, Some(&exec1.result),
    );
    let exec2 = r2.unwrap();
    assert_eq!(exec2.result, "6");
}

#[test]
fn test_pipe_output_to_stdin_stateful() {
    ensure_v8();

    // Step 1: stateful execution produces output + heap
    let (r1, _) = execute_stateful(
        "var counter = 10; counter;",
        None, HEAP, no_handle(), &[], HEAP, None,
    );
    let (exec1, _snap1, _hash1) = r1.unwrap();
    assert_eq!(exec1.result, "10");

    // Step 2: pipe output as stdin to fresh stateful execution
    let (r2, _) = execute_stateful(
        "var received = parseInt(__stdin__); received + 5;",
        None, HEAP, no_handle(), &[], HEAP, Some(&exec1.result),
    );
    let (exec2, _, _) = r2.unwrap();
    assert_eq!(exec2.result, "15");
}

#[test]
fn test_three_stage_pipeline() {
    ensure_v8();

    // Stage 1: generate data
    let (r1, _) = execute_stateless("10", HEAP, no_handle(), &[], HEAP, None);
    let o1 = r1.unwrap().result;

    // Stage 2: double it
    let (r2, _) = execute_stateless(
        "parseInt(__stdin__) * 2",
        HEAP, no_handle(), &[], HEAP, Some(&o1),
    );
    let o2 = r2.unwrap().result;

    // Stage 3: add 1
    let (r3, _) = execute_stateless(
        "parseInt(__stdin__) + 1",
        HEAP, no_handle(), &[], HEAP, Some(&o2),
    );
    let o3 = r3.unwrap().result;

    assert_eq!(o3, "21"); // 10 * 2 + 1
}

// ── Engine::run_js integration ──────────────────────────────────────────

#[tokio::test]
async fn test_engine_run_js_with_stdin() {
    ensure_v8();
    let engine = Engine::new_stateless(HEAP, 30, 4);

    let result = engine.run_js(
        "JSON.parse(__stdin__).x + 1".to_string(),
        None, None, None, None,
        Some(r#"{"x":41}"#.to_string()),
    ).await;

    assert!(result.is_ok(), "Should succeed, got: {:?}", result);
    let jr = result.unwrap();
    assert_eq!(jr.output, "42");
}

#[tokio::test]
async fn test_engine_run_js_console_log() {
    ensure_v8();
    let engine = Engine::new_stateless(HEAP, 30, 4);

    let result = engine.run_js(
        r#"console.log("hello"); console.error("oops"); "result""#.to_string(),
        None, None, None, None, None,
    ).await;

    assert!(result.is_ok());
    let jr = result.unwrap();
    assert_eq!(jr.output, "result");
    assert_eq!(jr.stdout, vec!["hello"]);
    assert_eq!(jr.stderr, vec!["oops"]);
}

#[tokio::test]
async fn test_engine_no_console_output_when_none() {
    ensure_v8();
    let engine = Engine::new_stateless(HEAP, 30, 4);

    let result = engine.run_js("1 + 1".to_string(), None, None, None, None, None).await;

    assert!(result.is_ok());
    let jr = result.unwrap();
    assert_eq!(jr.output, "2");
    assert!(jr.stdout.is_empty());
    assert!(jr.stderr.is_empty());
}

// ── Pipeline tests ────────────────────────────────────────────────────

fn code_stage(code: &str) -> PipelineStage {
    PipelineStage {
        code: Some(code.to_string()),
        heap: None,
        heap_memory_max_mb: None,
        execution_timeout_secs: None,
    }
}

fn heap_stage(hash: &str) -> PipelineStage {
    PipelineStage {
        code: None,
        heap: Some(hash.to_string()),
        heap_memory_max_mb: None,
        execution_timeout_secs: None,
    }
}

#[tokio::test]
async fn test_pipeline_code_only_three_stages() {
    ensure_v8();
    let engine = Engine::new_stateless(HEAP, 30, 4);

    let result = engine.run_pipeline(
        vec![
            code_stage("JSON.stringify([1, 2, 3, 4, 5])"),
            code_stage("JSON.stringify(JSON.parse(__stdin__).map(function(x) { return x * 2; }))"),
            code_stage("JSON.parse(__stdin__).reduce(function(a, b) { return a + b; }, 0)"),
        ],
        None,
    ).await;

    assert!(result.is_ok(), "Pipeline should succeed, got: {:?}", result);
    let pr = result.unwrap();
    assert_eq!(pr.stages.len(), 3);
    assert_eq!(pr.stages[0].output, "[1,2,3,4,5]");
    assert_eq!(pr.stages[1].output, "[2,4,6,8,10]");
    assert_eq!(pr.stages[2].output, "30");
}

#[tokio::test]
async fn test_pipeline_single_stage() {
    ensure_v8();
    let engine = Engine::new_stateless(HEAP, 30, 4);

    let result = engine.run_pipeline(vec![code_stage("42")], None).await;

    assert!(result.is_ok());
    let pr = result.unwrap();
    assert_eq!(pr.stages.len(), 1);
    assert_eq!(pr.stages[0].output, "42");
    assert_eq!(pr.stages[0].index, 0);
}

#[tokio::test]
async fn test_pipeline_empty_stages_error() {
    ensure_v8();
    let engine = Engine::new_stateless(HEAP, 30, 4);

    let result = engine.run_pipeline(vec![], None).await;

    assert!(result.is_err());
    assert!(result.unwrap_err().contains("at least one stage"));
}

#[tokio::test]
async fn test_pipeline_both_code_and_heap_error() {
    ensure_v8();
    let engine = Engine::new_stateless(HEAP, 30, 4);

    let result = engine.run_pipeline(
        vec![PipelineStage {
            code: Some("1".to_string()),
            heap: Some("abc".to_string()),
            heap_memory_max_mb: None,
            execution_timeout_secs: None,
        }],
        None,
    ).await;

    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not both"));
}

#[tokio::test]
async fn test_pipeline_neither_code_nor_heap_error() {
    ensure_v8();
    let engine = Engine::new_stateless(HEAP, 30, 4);

    let result = engine.run_pipeline(
        vec![PipelineStage {
            code: None,
            heap: None,
            heap_memory_max_mb: None,
            execution_timeout_secs: None,
        }],
        None,
    ).await;

    assert!(result.is_err());
    assert!(result.unwrap_err().contains("must provide code or heap"));
}

#[tokio::test]
async fn test_pipeline_stage_failure_reports_index() {
    ensure_v8();
    let engine = Engine::new_stateless(HEAP, 30, 4);

    let result = engine.run_pipeline(
        vec![
            code_stage("10"),
            code_stage("this_function_does_not_exist()"),
        ],
        None,
    ).await;

    assert!(result.is_err());
    assert!(result.unwrap_err().contains("stage 1"));
}

#[tokio::test]
async fn test_pipeline_output_pipes_to_stdin() {
    ensure_v8();
    let engine = Engine::new_stateless(HEAP, 30, 4);

    let result = engine.run_pipeline(
        vec![
            code_stage("10"),
            code_stage("parseInt(__stdin__) * 2"),
            code_stage("parseInt(__stdin__) + 1"),
        ],
        None,
    ).await;

    assert!(result.is_ok());
    let pr = result.unwrap();
    assert_eq!(pr.stages[0].output, "10");
    assert_eq!(pr.stages[1].output, "20");
    assert_eq!(pr.stages[2].output, "21");
}

#[tokio::test]
async fn test_pipeline_console_output_per_stage() {
    ensure_v8();
    let engine = Engine::new_stateless(HEAP, 30, 4);

    let result = engine.run_pipeline(
        vec![
            code_stage(r#"console.log("stage0"); "a""#),
            code_stage(r#"console.error("stage1err"); "b""#),
        ],
        None,
    ).await;

    assert!(result.is_ok());
    let pr = result.unwrap();
    assert_eq!(pr.stages[0].stdout, vec!["stage0"]);
    assert!(pr.stages[0].stderr.is_empty());
    assert!(pr.stages[1].stdout.is_empty());
    assert_eq!(pr.stages[1].stderr, vec!["stage1err"]);
}
