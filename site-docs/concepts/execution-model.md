# Execution Model

`mcp-v8` runs JavaScript in V8 but does not behave like a browser or a general
Node.js runtime. Code executes as ES modules, supports top-level `await`, and
uses an async execution registry to track background work in stateful mode.

The key design split is between:

- stateful execution, where `run_js` queues work and returns an execution ID
- stateless execution, where the server waits internally and returns output
  directly

Console output is streamed while code runs, and long-running work is governed
by per-execution timeout and memory limits.
