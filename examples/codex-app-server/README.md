# Driving the Codex `app-server` from `run_js`

This example connects mcp-v8's JavaScript runtime to the
[Codex `app-server`](https://github.com/openai/codex/blob/main/codex-rs/app-server/README.md)
— a bidirectional **JSON-RPC 2.0 over stdio** service (newline-delimited JSON,
same family as MCP) that powers the Codex VS Code extension.

It creates a thread and starts a `hello world` turn. Without Codex auth
configured, inference fails with `401 Unauthorized`, but **the thread is created
and persisted** to a rollout file on disk.

## Why this needs `spawn()` (not `output()`)

`Deno.Command#output()` / `child_process.exec()` run a command **to completion**
and hand back all of stdout at once. That cannot drive an interactive protocol,
because:

1. **The thread id is server-generated.** `turn/start` requires the `threadId`
   returned by `thread/start`, so a later request depends on an earlier
   response — impossible to express in one fire-and-forget batch.
2. **Codex only persists a thread once a turn runs.** `thread/start` alone does
   not write a rollout file; the session is written when the first turn starts.
   So the create-then-turn sequence must happen in **one live process**.
3. **The server flushes async responses only while stdin is open.** Closing
   stdin (EOF) makes it begin shutdown before responses are flushed.

The interactive `Deno.Command(...).spawn()` API (added alongside this example)
returns a `ChildProcess` that keeps the child alive and exposes incremental I/O:

| Method | Purpose |
|--------|---------|
| `await child.writeLine(str)` | write one JSON-RPC message + newline to stdin |
| `await child.readLine()` | read one line of stdout, or `null` at EOF |
| `await child.readStderrLine()` | read one line of stderr, or `null` at EOF |
| `await child.closeStdin()` | close stdin (send EOF) |
| `await child.kill()` | terminate the child |
| `await child.status()` | wait for exit; resolves to the exit code |

## Run it

1. **Install Codex** (prebuilt binary):
   ```bash
   npm install -g @openai/codex     # provides the `codex` binary with `app-server`
   ```

2. **Allow the spawn** via an OPA/Rego policy (subprocess is off by default):
   see [`../../policies/codex-app-server.rego`](../../policies/codex-app-server.rego).

3. **Start mcp-v8 with the policy enabled:**
   ```bash
   mcp-v8 --stateless --http-port 8080 \
     --policies-json '{"subprocess":{"policies":[{"url":"file:///ABS/PATH/policies/codex-app-server.rego"}]}}'
   ```

4. **Run the driver** as `run_js` code (via the REST sidecar, or as the `run_js`
   MCP tool `code` argument):
   ```bash
   curl --data-binary @driver.js -H 'Content-Type: application/javascript' \
     http://localhost:8080/api/exec
   ```

## Expected result

```jsonc
{
  "threadCreated": true,
  "threadId": "019f1eb8-…",
  "rolloutPath": "…/sessions/2026/07/01/rollout-…-019f1eb8-….jsonl",
  "inferenceError": {
    "message": "Reconnecting... 2/5",
    "codexErrorInfo": { "responseStreamDisconnected": { "httpStatusCode": 401 } }
  }
}
```

The rollout file on disk contains the `session_meta` and the `hello world` user
message, confirming the thread was created even though inference failed.

## No Codex changes required

The Codex `app-server` already speaks stdio JSON-RPC; nothing in Codex needs to
change. The only change is on the mcp-v8 side — the interactive `spawn()`
capability used above.
