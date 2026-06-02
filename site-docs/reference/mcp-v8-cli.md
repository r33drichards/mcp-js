# CLI Client Reference

`mcp-v8-cli` is the command-line client for the mcp-v8 HTTP API.

## Global Flags

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--url` | `string` | `http://localhost:3000` | Base URL of the mcp-v8 server |
| `--json` / `-j` | `bool` | `false` | Output raw JSON instead of pretty-printed text |

The `--url` flag can also be set via the `MCP_V8_URL` environment variable.

## Commands

### exec

Submit JavaScript code for asynchronous execution.

```
mcp-v8-cli exec <CODE> [OPTIONS]
```

| Argument/Flag | Type | Required | Description |
|---------------|------|----------|-------------|
| `CODE` | `string` | Yes | JavaScript or TypeScript code |
| `--heap` | `string` | No | Heap snapshot key to restore |
| `--session` | `string` | No | Session identifier |
| `--heap-memory-max-mb` | `u64` | No | V8 heap memory cap (MB) |
| `--execution-timeout-secs` | `i64` | No | Execution timeout (seconds) |
| `--tag` | `string` (repeatable) | No | Key=value tag pair |

**Example:**

```
mcp-v8-cli exec "console.log('hello')" --tag env=prod --tag version=1.0
```

### executions list

List all known executions.

```
mcp-v8-cli executions list
```

### executions get

Get the status and result of an execution.

```
mcp-v8-cli executions get <ID>
```

| Argument | Type | Required | Description |
|----------|------|----------|-------------|
| `ID` | `string` | Yes | Execution ID |

### executions output

Read paginated console output from an execution.

```
mcp-v8-cli executions output <ID> [OPTIONS]
```

| Argument/Flag | Type | Required | Description |
|---------------|------|----------|-------------|
| `ID` | `string` | Yes | Execution ID |
| `--line-offset` | `i64` | No | Start line (0-indexed) |
| `--line-limit` | `i64` | No | Max lines to return |
| `--byte-offset` | `i64` | No | Start byte offset |
| `--byte-limit` | `i64` | No | Max bytes to return |

When more output is available, the command prints a hint to stderr:

```
[more output available -- next_line_offset=101 next_byte_offset=4096]
```

### executions cancel

Cancel a running execution.

```
mcp-v8-cli executions cancel <ID>
```

| Argument | Type | Required | Description |
|----------|------|----------|-------------|
| `ID` | `string` | Yes | Execution ID |

## Environment Variables

| Variable | Description |
|----------|-------------|
| `MCP_V8_URL` | Base URL of the mcp-v8 server (default: `http://localhost:3000`) |

## Output Formats

By default, the CLI displays human-readable formatted output. Pass `--json` or `-j` for raw JSON output suitable for scripting.

### Human-readable output examples

**exec:**

```
Execution queued
   execution_id: a1b2c3d4-e5f6-4789-abcd-ef0123456789

Poll status:  mcp-v8-cli --url http://localhost:3000 executions get a1b2c3d4-...
Read output:  mcp-v8-cli --url http://localhost:3000 executions output a1b2c3d4-...
```

**executions list:**

```
EXECUTION ID                           STATUS       STARTED AT                 COMPLETED AT
----------------------------------------------------------------------------------------------------
a1b2c3d4-e5f6-4789-abcd-ef0123456789  completed    2024-01-01T00:00:00Z       2024-01-01T00:00:01Z
```

**executions get:**

```
execution_id : a1b2c3d4-e5f6-4789-abcd-ef0123456789
status       : completed
started_at   : 2024-01-01T00:00:00Z
completed_at : 2024-01-01T00:00:01Z
result       :
heap         : a3f2b8c1...
```
