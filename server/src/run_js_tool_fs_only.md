run javascript or typescript code in v8

Submits code for **async execution** — returns an execution ID immediately. V8 runs in the background. Use `get_execution` to poll status and result, `get_execution_output` to read console output, and `cancel_execution` to stop a running execution.

This server has a **persistent per-session filesystem** but **no heap persistence**: JavaScript globals/variables do NOT survive between calls. Persist anything you need across calls by writing it to the `/work` filesystem (`await fs.writeFile('/work/state.json', ...)`), which is content-addressed and restored automatically for the same session on the next run.

TypeScript support is type removal only — types are stripped before execution, not checked. Invalid types will be silently removed, not reported as errors.

params:
- code (optional): the javascript or typescript code to run. Provide either `code` or `file`.
- file (optional): path to a JavaScript/TypeScript file **on the server's own filesystem** to read and execute instead of inline `code`. Provide either `code` or `file`, not both. Disabled unless the server was started with `--allow-run-js-file` or a `run_js_file` policy.
- fs (optional): a filesystem snapshot to mount (a label or a CA id). Omit to continue this session's filesystem, which is mounted automatically.
- heap_memory_max_mb (optional): maximum V8 heap memory in megabytes (minimum: 4, default: 8). Override the server default for this execution.
- execution_timeout_secs (optional): maximum execution time in seconds (1–300, default: 30). Override the server default for this execution.

returns:
- execution_id: UUID of the submitted execution. Use with get_execution, get_execution_output, and cancel_execution. `get_execution` also returns `fs` — the resulting filesystem snapshot CA id.

Session identity comes from the `X-MCP-Session-Id` header during initialization, not from a `session` tool parameter. The session's latest `/work` filesystem is mounted automatically on each run and the result is snapshotted, so files persist across calls without tracking CA ids.

## Console Output

Use `console.log()` to produce output. `console.info`, `console.warn`, and `console.error` are also supported (with `[INFO]`, `[WARN]`, `[ERROR]` prefixes respectively).
