# mcp-v8: V8 JavaScript MCP Server

A Rust-based Model Context Protocol (MCP) server that exposes a V8 JavaScript runtime as a tool for AI agents like Claude and Cursor. Supports persistent heap snapshots via S3 or local filesystem, and is ready for integration with modern AI development environments.

## Features

- **Async Execution Model**: `run_js` returns immediately with an execution ID. Poll status with `get_execution`, read console output with `get_execution_output`, and cancel running executions with `cancel_execution`.
- **Console Output**: Full support for `console.log`, `console.info`, `console.warn`, and `console.error`. Output is streamed to persistent storage during execution and can be read in real-time with paginated access (line-based or byte-based).
- **Async/Await Support**: Full support for `async`/`await` and Promises via the deno_core event loop.
- **V8 JavaScript Execution**: Run arbitrary JavaScript code in a secure, isolated V8 engine.
- **TypeScript Support**: Run TypeScript code directly — types are stripped before execution using [SWC](https://swc.rs/). This is type removal only, not type checking.
- **WebAssembly Support**: Compile and run WASM modules using the standard `WebAssembly` JavaScript API (`WebAssembly.Module`, `WebAssembly.Instance`, `WebAssembly.validate`).
- **Content-Addressed Heap Snapshots**: Persist and restore V8 heap state between runs using content-addressed storage, supporting both S3 and local file storage.
- **Stateless Mode**: Optional mode for fresh executions without heap persistence, ideal for serverless environments.
- **MCP Protocol**: Implements the Model Context Protocol for seamless tool integration with Claude, Cursor, and other MCP clients.
- **Configurable Storage**: Choose between S3, local directory, or stateless mode at runtime.
- **Multiple Transports**: Supports stdio, Streamable HTTP (MCP 2025-03-26+), and SSE (Server-Sent Events) transport protocols.
- **Clustering**: Optional Raft-based clustering for distributed coordination, replicated session logging, and horizontal scaling.
- **Concurrency Control**: Configurable concurrent V8 execution limits with semaphore-based throttling.
- **OPA-Gated Fetch**: Optional `fetch()` function for JavaScript following the web standard Fetch API, with every HTTP request checked against an [Open Policy Agent](https://www.openpolicyagent.org/) policy before execution.
- **Fetch Header Injection**: Automatically inject headers (e.g., auth tokens, API keys) into outgoing fetch requests based on host and method matching rules, configured via CLI flags or a JSON config file.

## Installation

Install `mcp-v8` using the provided install script:

```bash
curl -fsSL https://raw.githubusercontent.com/r33drichards/mcp-js/main/install.sh | sudo bash
```

This will automatically download and install the latest release for your platform to `/usr/local/bin/mcp-v8` (you may be prompted for your password).

---

*Advanced users: If you prefer to build from source, see the [Build from Source](#build-from-source) section at the end of this document.*

## Command Line Arguments

`mcp-v8` supports the following command line arguments:

### Storage Options

- `--s3-bucket <bucket>`: Use AWS S3 for heap snapshots. Specify the S3 bucket name. (Conflicts with `--stateless`)
- `--cache-dir <path>`: Local filesystem cache directory for S3 write-through caching. Reduces latency by caching snapshots locally. (Requires `--s3-bucket`)
- `--directory-path <path>`: Use a local directory for heap snapshots. Specify the directory path. (Conflicts with `--stateless`)
- `--stateless`: Run in stateless mode - no heap snapshots are saved or loaded. Each JavaScript execution starts with a fresh V8 isolate. (Conflicts with `--s3-bucket` and `--directory-path`)

**Note:** For heap storage, if neither `--s3-bucket`, `--directory-path`, nor `--stateless` is provided, the server defaults to using `/tmp/mcp-v8-heaps` as the local directory.

### Transport Options

- `--http-port <port>`: Enable Streamable HTTP transport (MCP 2025-03-26+) on the specified port. Serves the MCP endpoint at `/mcp` and a plain API at `/api/exec`. If not provided, the server uses stdio transport (default). (Conflicts with `--sse-port`)
- `--sse-port <port>`: Enable SSE (Server-Sent Events) transport on the specified port. Exposes `/sse` for the event stream and `/message` for client requests. (Conflicts with `--http-port`)

### Execution Limits

- `--heap-memory-max <megabytes>`: Maximum V8 heap memory per isolate in megabytes (1–64, default: 8).
- `--execution-timeout <seconds>`: Maximum execution timeout in seconds (1–300, default: 30).
- `--max-concurrent-executions <n>`: Maximum number of concurrent V8 executions (default: CPU core count). Controls how many JavaScript executions can run in parallel.

### Session Logging

- `--session-db-path <path>`: Path to the sled database used for session logging (default: `/tmp/mcp-v8-sessions`). Only applies in stateful mode. (Conflicts with `--stateless`)

### Cluster Options

These options enable Raft-based clustering for distributed coordination and replicated session logging.

- `--cluster-port <port>`: Port for the Raft cluster HTTP server. Enables cluster mode when set. (Requires `--http-port` or `--sse-port`)
- `--node-id <id>`: Unique node identifier within the cluster (default: `node1`).
- `--peers <peers>`: Comma-separated list of seed peer addresses. Format: `id@host:port` or `host:port`. Peers can also join dynamically via `POST /raft/join`.
- `--join <address>`: Join an existing cluster by contacting this seed address (`host:port`). The node registers itself with the cluster leader.
- `--advertise-addr <addr>`: Advertise address for this node (`host:port`). Used for peer discovery and write forwarding. Defaults to `<node-id>:<cluster-port>`.
- `--heartbeat-interval <ms>`: Raft heartbeat interval in milliseconds (default: 100).
- `--election-timeout-min <ms>`: Minimum election timeout in milliseconds (default: 300).
- `--election-timeout-max <ms>`: Maximum election timeout in milliseconds (default: 500).

### OPA / Fetch Options

These options enable an OPA-gated `fetch()` function in the JavaScript runtime. When `--opa-url` is set, a `fetch(url, opts?)` global becomes available. `fetch()` follows the web standard Fetch API — it returns a Promise that resolves to a Response object. Every outbound HTTP request is first checked against an OPA policy — the request is only made if the policy returns `{"allow": true}`.

- `--opa-url <URL>`: OPA server URL (e.g. `http://localhost:8181`). Enables `fetch()` in the JS runtime.
- `--opa-fetch-policy <path>`: OPA policy path appended to `/v1/data/` (default: `mcp/fetch`). Requires `--opa-url`.
- `--fetch-header <RULE>`: Inject headers into fetch requests matching host/method rules. Format: `host=<host>,header=<name>,value=<val>[,methods=GET;POST]`. Can be specified multiple times. Requires `--opa-url`. See [Fetch Header Injection](#fetch-header-injection) for details.
- `--fetch-header-config <PATH>`: Path to a JSON file with header injection rules. Format: `[{"host": "...", "methods": [...], "headers": {...}}]`. Requires `--opa-url`. See [Fetch Header Injection](#fetch-header-injection) for details.

**Example:**
```bash
mcp-v8 --stateless --http-port 3000 \
  --opa-url http://localhost:8181 \
  --opa-fetch-policy mcp/fetch
```

### WASM Module Options

Pre-load WebAssembly modules that are available as global variables in every JavaScript execution.

- `--wasm-module <name>=<path>`: Pre-load a `.wasm` file and expose its exports as a global variable with the given name. Can be specified multiple times for multiple modules.
- `--wasm-config <path>`: Path to a JSON config file mapping global names to `.wasm` file paths. Format: `{"name": "/path/to/module.wasm", ...}`.

Both options can be used together. CLI flags and config file entries are merged; duplicate names cause an error.

**Example — CLI flags:**
```bash
mcp-v8 --stateless --wasm-module math=/path/to/math.wasm --wasm-module crypto=/path/to/crypto.wasm
```

**Example — Config file** (`wasm-modules.json`):
```json
{
  "math": "/path/to/math.wasm",
  "crypto": "/path/to/crypto.wasm"
}
```
```bash
mcp-v8 --stateless --wasm-config wasm-modules.json
```

After loading, the module exports are available directly in JavaScript:
```javascript
math.add(21, 21); // → 42
```

**Modules with imports** (e.g. WASI modules like SQLite) are also supported. When a module has imports, auto-instantiation is skipped and the compiled `WebAssembly.Module` is exposed as `__wasm_<name>`. Your JavaScript code can then instantiate it with the required imports:
```javascript
// __wasm_sqlite is the compiled WebAssembly.Module
var instance = new WebAssembly.Instance(__wasm_sqlite, {
    wasi_snapshot_preview1: { /* WASI stubs */ },
});
instance.exports.sqlite3_open(/* ... */);
```

Self-contained modules (no imports) are auto-instantiated as before — their exports are set directly on `<name>`, and the compiled Module is also available as `__wasm_<name>`.

See the [SQLite WASM example](examples/sqlite-wasm/) for a complete working example.

## Quick Start

After installation, you can run the server directly. Choose one of the following options:

### Stdio Transport (Default)

```bash
# Use S3 for heap storage (recommended for cloud/persistent use)
mcp-v8 --s3-bucket my-bucket-name

# Use local filesystem directory for heap storage (recommended for local development)
mcp-v8 --directory-path /tmp/mcp-v8-heaps

# Use stateless mode - no heap persistence (recommended for one-off computations)
mcp-v8 --stateless
```

### HTTP Transport (Streamable HTTP)

The HTTP transport uses the Streamable HTTP protocol (MCP 2025-03-26+), which supports bidirectional communication over standard HTTP. The MCP endpoint is served at `/mcp`:

```bash
# Start HTTP server on port 8080 with local filesystem storage
mcp-v8 --directory-path /tmp/mcp-v8-heaps --http-port 8080

# Start HTTP server on port 8080 with S3 storage
mcp-v8 --s3-bucket my-bucket-name --http-port 8080

# Start HTTP server on port 8080 in stateless mode
mcp-v8 --stateless --http-port 8080
```

The HTTP transport also exposes a plain HTTP API at `POST /api/exec` for direct JavaScript execution without MCP framing.

The HTTP transport is useful for:
- Network-based MCP clients
- Load-balanced and horizontally-scaled deployments
- Testing and debugging with tools like the MCP Inspector
- Containerized deployments
- Remote MCP server access

### SSE Transport

Server-Sent Events (SSE) transport for streaming responses:

```bash
# Start SSE server on port 8081 with local filesystem storage
mcp-v8 --directory-path /tmp/mcp-v8-heaps --sse-port 8081

# Start SSE server on port 8081 in stateless mode
mcp-v8 --stateless --sse-port 8081
```

## MCP Tools

### Execution Workflow

`run_js` uses an **async execution model** — it submits code for background execution and returns an execution ID immediately. Use `get_execution` to poll for completion and retrieve the result, and `get_execution_output` to read console output.

```
1. run_js(code)           → { execution_id }
2. get_execution(id)      → { status: "running" | "completed" | "failed" | "cancelled" | "timed_out", result, error }
3. get_execution_output(id, line_offset, line_limit)  → paginated console output
```

**Example:**

```
Call run_js with code: "console.log('hello'); 1 + 1;"
  → { execution_id: "abc-123" }

Call get_execution with execution_id: "abc-123"
  → { status: "completed", result: "2" }

Call get_execution_output with execution_id: "abc-123"
  → { data: "hello\n", total_lines: 1 }
```

### Return Values

The **last expression** in your code is the return value, available via `get_execution` once the execution completes:

```javascript
const result = 1 + 1;
result;
// → result: "2"
```

Objects must be serialized to see their contents:

```javascript
const obj = { a: 1, b: 2 };
JSON.stringify(obj);
// → result: '{"a":1,"b":2}'
```

### Console Output

`console.log`, `console.info`, `console.warn`, and `console.error` are fully supported. Output is streamed to persistent storage during execution and can be read in real-time using `get_execution_output`.

`get_execution_output` supports two pagination modes:

- **Line mode**: `line_offset` + `line_limit` — fetch N lines starting from line M
- **Byte mode**: `byte_offset` + `byte_limit` — fetch N bytes starting from byte M

Both modes return position info in both coordinate systems for cross-referencing. Use `next_line_offset` or `next_byte_offset` from a response to resume reading.

### Tools (All Modes)

| Tool | Description |
|------|-------------|
| `run_js` | Submit JavaScript/TypeScript code for async execution. Returns an `execution_id` immediately. Parameters: `code` (required), `heap_memory_max_mb` (optional, 4–64, default: 8), `execution_timeout_secs` (optional, 1–300, default: 30). |
| `get_execution` | Poll execution status and result. Returns `execution_id`, `status`, `result` (if completed), `error` (if failed), `started_at`, `completed_at`. |
| `get_execution_output` | Read paginated console output. Supports line-based (`line_offset` + `line_limit`) or byte-based (`byte_offset` + `byte_limit`) pagination. |
| `cancel_execution` | Terminate a running V8 execution. |
| `list_executions` | List all executions with their status. |

### Additional Tools (Stateful Mode Only)

In stateful mode, `run_js` accepts additional parameters: `heap` (SHA-256 hash to resume from) and `session` (human-readable session name for logging).

| Tool | Description |
|------|-------------|
| `list_sessions` | List all named sessions. |
| `list_session_snapshots` | Browse execution history for a session. Accepts `session` (required) and `fields` (optional, comma-separated: `index`, `input_heap`, `output_heap`, `code`, `timestamp`). |
| `get_heap_tags` | Get tags for a heap snapshot. |
| `set_heap_tags` | Set or replace tags on a heap snapshot. |
| `delete_heap_tags` | Delete specific tag keys from a heap snapshot. |
| `query_heaps_by_tags` | Find heap snapshots matching tag criteria. |

## Stateless vs Stateful Mode

### Stateless Mode (`--stateless`)

Stateless mode runs each JavaScript execution in a fresh V8 isolate without any heap persistence.

**Benefits:**
- **Faster execution**: No snapshot creation/serialization overhead
- **No storage I/O**: Doesn't read or write heap files
- **Fresh isolates**: Every JS execution starts clean
- **Perfect for**: One-off computations, stateless functions, serverless environments

**Example use case:** Simple calculations, data transformations, or any scenario where you don't need to persist state between executions.

### Stateful Mode (default)

Stateful mode persists the V8 heap state between executions using content-addressed storage backed by either S3 or local filesystem.

Each execution returns a `heap` content hash (a 64-character SHA-256 hex string) that identifies the snapshot. Pass this hash in the next `run_js` call to resume from that state. Omit `heap` to start a fresh session.

**Benefits:**
- **State persistence**: Variables and objects persist between runs
- **Content-addressed**: Snapshots are keyed by their SHA-256 hash — no naming collisions, safe concurrent access, and natural deduplication
- **Immutable snapshots**: Once written, a snapshot at a given hash never changes
- **Perfect for**: Interactive sessions, building up complex state over time

**Example use case:** Building a data structure incrementally, maintaining session state, or reusing expensive computations.

#### Named Sessions

You can tag executions with a human-readable **session name** by passing the `session` parameter to `run_js`. When a session name is provided, the server logs each execution (input heap, output heap, code, and timestamp) to an embedded sled database.

Two additional tools are available in stateful mode for browsing session history:

- **`list_sessions`** — Returns an array of all session names that have been used.
- **`list_session_snapshots`** — Returns the log entries for a given session. Accepts a required `session` parameter and an optional `fields` parameter (comma-separated) to select specific fields: `index`, `input_heap`, `output_heap`, `code`, `timestamp`.

The session database path defaults to `/tmp/mcp-v8-sessions` and can be overridden with `--session-db-path`.

**Example workflow:**

1. Call `run_js` with `code: "var x = 1; x;"` and `session: "my-project"` → receives `execution_id`.
2. Call `get_execution` with the `execution_id` → receives `{ status: "completed", result: "1", heap: "ab12..." }`.
3. Pass the returned `heap` hash and `session: "my-project"` in subsequent `run_js` calls to continue and log the session.
4. Call `list_sessions` to see `["my-project"]`.
5. Call `list_session_snapshots` with `session: "my-project"` to see the full execution history.

## Integration

### Claude for Desktop

1. Install the server as above.
2. Open Claude Desktop → Settings → Developer → Edit Config.
3. Add your server to `claude_desktop_config.json`:

**Stateful mode with S3:**
```json
{
  "mcpServers": {
    "js": {
      "command": "mcp-v8",
      "args": ["--s3-bucket", "my-bucket-name"]
    }
  }
}
```

**Stateful mode with local filesystem:**
```json
{
  "mcpServers": {
    "js": {
      "command": "mcp-v8",
      "args": ["--directory-path", "/tmp/mcp-v8-heaps"]
    }
  }
}
```

**Stateless mode:**
```json
{
  "mcpServers": {
    "js": {
      "command": "mcp-v8",
      "args": ["--stateless"]
    }
  }
}
```

4. Restart Claude Desktop. The new tools will appear under the hammer icon.

### Claude Code CLI

Add the MCP server to Claude Code using the `claude mcp add` command:

**Stdio transport (local):**
```bash
# Stateful mode with local filesystem
claude mcp add mcp-v8 -- mcp-v8 --directory-path /tmp/mcp-v8-heaps

# Stateless mode
claude mcp add mcp-v8 -- mcp-v8 --stateless
```

**SSE transport (remote):**
```bash
claude mcp add mcp-v8 -t sse https://mcp-js-production.up.railway.app/sse
```

Then test by running `claude` and asking: "Run this JavaScript: `[1,2,3].map(x => x * 2)`"

### Cursor

1. Install the server as above.
2. Create or edit `.cursor/mcp.json` in your project root:

**Stateful mode with local filesystem:**
```json
{
  "mcpServers": {
    "js": {
      "command": "mcp-v8",
      "args": ["--directory-path", "/tmp/mcp-v8-heaps"]
    }
  }
}
```

**Stateless mode:**
```json
{
  "mcpServers": {
    "js": {
      "command": "mcp-v8",
      "args": ["--stateless"]
    }
  }
}
```

3. Restart Cursor. The MCP tools will be available in the UI.

### Claude (Web/Cloud) via Railway

You can also use the hosted version on Railway without installing anything locally:

1. Go to Claude's connectors settings page
2. Add a new custom connector:
   - **Name**: "mcp-v8"
   - **URL**: `https://mcp-js-production.up.railway.app/sse`

## Example Usage

Ask Claude or Cursor: "Run this JavaScript: `1 + 2`"

The agent will:
1. Call `run_js` with `code: "1 + 2"` → receives `execution_id`
2. Call `get_execution` with the `execution_id` → receives `{ status: "completed", result: "3" }`

In stateful mode, `get_execution` also returns a `heap` content hash — pass it back in the next `run_js` call to resume from that state.

### WebAssembly

You can compile and run WebAssembly modules using the standard `WebAssembly` JavaScript API:

```javascript
const wasmBytes = new Uint8Array([
  0x00,0x61,0x73,0x6d, // magic
  0x01,0x00,0x00,0x00, // version
  0x01,0x07,0x01,0x60,0x02,0x7f,0x7f,0x01,0x7f, // type: (i32,i32)->i32
  0x03,0x02,0x01,0x00, // function section
  0x07,0x07,0x01,0x03,0x61,0x64,0x64,0x00,0x00, // export "add"
  0x0a,0x09,0x01,0x07,0x00,0x20,0x00,0x20,0x01,0x6a,0x0b // body: local.get 0, local.get 1, i32.add
]);
const mod = new WebAssembly.Module(wasmBytes);
const inst = new WebAssembly.Instance(mod);
inst.exports.add(21, 21); // → 42
```

Both synchronous (`WebAssembly.Module` / `WebAssembly.Instance`) and async (`WebAssembly.compile`, `WebAssembly.instantiate`) WebAssembly APIs are supported. The runtime resolves Promises automatically via the event loop.

Alternatively, you can pre-load `.wasm` files at server startup using `--wasm-module` or `--wasm-config` so they are available as globals in every execution without inline byte arrays. See [WASM Module Options](#wasm-module-options) for details.

### SQLite WASM

Run a full SQLite database inside mcp-v8 using [SQLite WASM](https://github.com/sqlite/sqlite-wasm):

```bash
# Build the WASM module (requires Emscripten)
./examples/sqlite-wasm/build.sh

# Start the server with SQLite pre-loaded
mcp-v8 --stateless --wasm-module sqlite=examples/sqlite-wasm/sqlite3.wasm
```

Then run SQL from JavaScript:

```javascript
var db = new SQLite();
db.exec("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, age INTEGER)");
db.exec("INSERT INTO users (name, age) VALUES ('Alice', 30)");
var result = db.query("SELECT * FROM users");
db.close();
JSON.stringify(result.rows);  // → [{"id":1,"name":"Alice","age":30}]
```

See [`examples/sqlite-wasm/`](examples/sqlite-wasm/) for the full example including the SQLite wrapper code and WASI import stubs.

### OPA-Gated Fetch

When the server is started with `--opa-url`, JavaScript code can use a `fetch(url, opts?)` function following the web standard Fetch API. Every request is checked against an OPA policy before the HTTP call is made.

**1. Write an OPA policy**

Create a Rego policy that controls which requests are allowed:

```rego
package mcp.fetch

default allow = false

# Allow GET requests to a specific API host
allow if {
    input.method == "GET"
    input.url_parsed.host == "api.example.com"
    startswith(input.url_parsed.path, "/public/")
}
```

The policy input includes:
- `operation`: always `"fetch"`
- `url`: the full URL string
- `method`: HTTP method (e.g. `"GET"`, `"POST"`)
- `headers`: request headers (keys normalized to lowercase)
- `url_parsed`: parsed URL components — `scheme`, `host`, `port`, `path`, `query`

**2. Start the server with OPA enabled**

```bash
mcp-v8 --stateless --http-port 3000 \
  --opa-url http://localhost:8181 \
  --opa-fetch-policy mcp/fetch
```

**3. Use `fetch()` in JavaScript**

```javascript
const resp = await fetch("https://api.example.com/public/data");
resp.status;              // 200
resp.ok;                  // true
await resp.text();        // response body as string
await resp.json();        // parsed JSON
resp.headers.get("content-type"); // header value
```

The response object supports:
- Properties: `.ok`, `.status`, `.statusText`, `.url`, `.redirected`, `.type`, `.bodyUsed`
- Methods: `.text()`, `.json()`, `.clone()` (`.text()` and `.json()` return Promises)
- Headers: `.headers.get(name)`, `.headers.has(name)`, `.headers.entries()`, `.headers.keys()`, `.headers.values()`, `.headers.forEach(fn)`

`fetch()` also accepts an options object:

```javascript
const resp = await fetch("https://api.example.com/data", {
  method: "POST",
  headers: { "Content-Type": "application/json" },
  body: JSON.stringify({ key: "value" })
});
JSON.stringify(await resp.json());
```

If the OPA policy denies a request, the Promise returned by `fetch()` is rejected with an error.

### Fetch Header Injection

When using OPA-gated fetch, you can configure automatic header injection rules that add headers to outgoing `fetch()` requests based on the target host and HTTP method. This is useful for injecting authentication tokens, API keys, or other credentials without embedding them in JavaScript code.

Header injection rules are evaluated per-request. If a rule's host pattern and method filter match, its headers are injected into the request. **User-provided headers always take precedence** — a rule will not overwrite a header that JavaScript code already set.

#### CLI Flags (`--fetch-header`)

Use `--fetch-header` to define rules inline. The format is:

```
host=<host>,header=<name>,value=<val>[,methods=GET;POST]
```

- `host` — Host to match (exact or wildcard, see below). **Required.**
- `header` — Header name to inject. **Required.**
- `value` — Header value to inject. **Required.**
- `methods` — Semicolon-separated HTTP methods to match. **Optional.** If omitted, the rule applies to all methods.

Can be specified multiple times for multiple rules:

```bash
mcp-v8 --stateless --opa-url http://localhost:8181 \
  --fetch-header "host=api.github.com,header=Authorization,value=Bearer ghp_xxxx" \
  --fetch-header "host=api.example.com,header=X-API-Key,value=secret123"
```

#### JSON Config File (`--fetch-header-config`)

For managing many rules, use a JSON config file. Each rule can inject multiple headers at once:

```json
[
  {
    "host": "api.github.com",
    "methods": ["GET", "POST"],
    "headers": {
      "Authorization": "Bearer ghp_xxxx",
      "X-GitHub-Api-Version": "2022-11-28"
    }
  },
  {
    "host": "*.example.com",
    "headers": {
      "X-API-Key": "secret123"
    }
  }
]
```

```bash
mcp-v8 --stateless --opa-url http://localhost:8181 \
  --fetch-header-config headers.json
```

Both `--fetch-header` and `--fetch-header-config` can be used together — their rules are merged.

#### Host Matching

- **Exact match**: `api.github.com` matches only `api.github.com`.
- **Wildcard**: `*.github.com` matches `api.github.com`, `github.com`, and `sub.api.github.com`.
- Host matching is **case-insensitive**.

#### Header Precedence

Headers set explicitly in JavaScript `fetch()` calls always win. Injection rules only add headers that are not already present:

```javascript
// Rule: host=api.example.com, header=Authorization, value=Bearer injected

// Header is injected (not set by code):
await fetch("https://api.example.com/data");
// → request includes Authorization: Bearer injected

// User header takes precedence:
await fetch("https://api.example.com/data", {
  headers: { "Authorization": "Bearer my-own-token" }
});
// → request includes Authorization: Bearer my-own-token (rule skipped)
```

### Loading `.wasm` Files

Instead of embedding raw bytes in JavaScript, you can compile a `.wasm` file once and load it at server startup. The module's exports are then available as a global variable in every execution.

**1. Create a WASM module**

Write a WebAssembly Text Format (`.wat`) file and compile it with [`wat2wasm`](https://github.com/WebAssembly/wabt):

```wat
;; math.wat — exports add(i32, i32) -> i32
(module
  (func $add (param i32 i32) (result i32)
    local.get 0
    local.get 1
    i32.add)
  (export "add" (func $add)))
```

```bash
wat2wasm math.wat -o math.wasm
```

**2. Start the server with the module**

```bash
# Single module
mcp-v8 --stateless --wasm-module math=./math.wasm

# Multiple modules
mcp-v8 --stateless \
  --wasm-module math=./math.wasm \
  --wasm-module physics=./physics.wasm
```

Or use a JSON config file for many modules:

```json
{
  "math": "./math.wasm",
  "physics": "./physics.wasm"
}
```

```bash
mcp-v8 --stateless --wasm-config wasm-modules.json
```

**3. Call exports from JavaScript**

The module's exports are available directly on the global variable:

```javascript
math.add(2, 3);          // → 5
math.add(100, 200);      // → 300
```

When multiple modules are loaded, each is its own global:

```javascript
var sum = math.add(10, 5);           // 15
var product = physics.multiply(3, 4); // 12
sum + product;                        // → 27
```

## Heap Storage Options

You can configure heap storage using the following command line arguments:

- **S3**: `--s3-bucket <bucket>`
  - Example: `mcp-v8 --s3-bucket my-bucket-name`
  - Requires AWS credentials in your environment.
  - Ideal for cloud deployments and sharing state across instances.
- **S3 with write-through cache**: `--s3-bucket <bucket> --cache-dir <path>`
  - Example: `mcp-v8 --s3-bucket my-bucket-name --cache-dir /tmp/mcp-v8-cache`
  - Reads from local cache first, writes to both local cache and S3.
  - Reduces latency for repeated snapshot access.
- **Filesystem**: `--directory-path <path>`
  - Example: `mcp-v8 --directory-path /tmp/mcp-v8-heaps`
  - Stores heap snapshots locally on disk.
  - Ideal for local development and testing.
- **Stateless**: `--stateless`
  - Example: `mcp-v8 --stateless`
  - No heap persistence - each execution starts fresh.
  - Ideal for one-off computations and serverless environments.

**Note:** Only one storage option can be used at a time. If multiple are provided, the server will return an error.

## Limitations

While `mcp-v8` provides a powerful and persistent JavaScript execution environment, there are limitations to its runtime.

- **Async execution model**: `run_js` returns immediately with an execution ID. Use `get_execution` to poll for the result and `get_execution_output` to read console output. Each execution runs in a fresh V8 isolate — no state is shared between calls (unless using stateful mode with heap snapshots).
- **`async`/`await` and Promises**: Fully supported. If your code returns a Promise, the runtime resolves it automatically.
- **`console.log` supported**: `console.log`, `console.info`, `console.warn`, and `console.error` are captured and available via `get_execution_output` with paginated access.
- **No `fetch` or network access by default**: When the server is started with `--opa-url`, a `fetch(url, opts?)` function becomes available following the web standard Fetch API. Each request is checked against an OPA policy before execution. Without `--opa-url`, there is no network access. See [OPA-Gated Fetch](#opa-gated-fetch) for details.
- **No file system access**: The runtime does not provide access to the local file system or environment variables.
- **No `npm install` or external packages**: You cannot install or import npm packages. Only standard JavaScript (ECMAScript) built-ins are available.
- **No timers**: Functions like `setTimeout` and `setInterval` are not available.
- **No DOM or browser APIs**: This is not a browser environment; there is no access to `window`, `document`, or other browser-specific objects.
- **WebAssembly**: Both synchronous (`WebAssembly.Module`, `WebAssembly.Instance`, `WebAssembly.validate`) and async (`WebAssembly.compile`, `WebAssembly.instantiate`) APIs are supported.
- **TypeScript: type removal only**: TypeScript type annotations are stripped before execution. No type checking is performed — invalid types are silently removed, not reported as errors.

---

## Build from Source (Advanced)

If you prefer to build from source instead of using the install script:

### Prerequisites
- Rust (nightly toolchain recommended)
- (Optional) AWS credentials for S3 storage

### Build the Server

```bash
cd server
cargo build --release
```

The built binary will be located at `server/target/release/server`. You can use this path in the integration steps above instead of `/usr/local/bin/mcp-v8` if desired.

<!-- load-test-report -->
# MCP-V8 Load Test Benchmark Report v0.1.0

Comparison of single-node vs 3-node cluster at various request rates.

## Results

ran on railway gha runners on [pr](https://github.com/r33drichards/mcp-js/pull/36#issuecomment-3946074130)

| Topology | Target Rate | Actual Iter/s | HTTP Req/s | Exec Avg (ms) | Exec p95 (ms) | Exec p99 (ms) | Success % | Dropped | Max VUs |
|----------|-------------|---------------|------------|----------------|----------------|----------------|-----------|---------|---------|
| cluster-stateful | 100/s | 99.5 | 99.5 | 44.9 | 196.88 | 416.99 | 100% | 31 | 41 |
| cluster-stateful | 200/s | 199.6 | 199.6 | 23.22 | 79.32 | 131.13 | 100% | 13 | 33 |
| cluster-stateless | 1000/s | 999.9 | 999.9 | 3.82 | 7.72 | 13.09 | 100% | 0 | 100 |
| cluster-stateless | 100/s | 100 | 100 | 3.67 | 5.65 | 8.03 | 100% | 0 | 10 |
| cluster-stateless | 200/s | 200 | 200 | 3.56 | 5.9 | 8.61 | 100% | 0 | 20 |
| cluster-stateless | 500/s | 500 | 500 | 3.42 | 5.85 | 9.2 | 100% | 0 | 50 |
| single-stateful | 100/s | 99.1 | 99.1 | 215.12 | 362.5 | 376.6 | 100% | 32 | 42 |
| single-stateful | 200/s | 97.8 | 97.8 | 1948.82 | 2212.55 | 2960.96 | 100% | 5939 | 200 |
| single-stateless | 1000/s | 977.1 | 977.1 | 60.98 | 482.98 | 602.38 | 100% | 843 | 561 |
| single-stateless | 100/s | 100 | 100 | 3.71 | 5.73 | 8.73 | 100% | 0 | 10 |
| single-stateless | 200/s | 200 | 200 | 3.61 | 5.43 | 7.74 | 100% | 0 | 20 |
| single-stateless | 500/s | 500 | 500 | 4.67 | 8.49 | 27.98 | 100% | 0 | 50 |

## P95 Latency

| Topology | Rate | P95 (ms) | |
|----------|------|----------|-|
| cluster-stateful | 100/s | 196.88 | `█████████████████████` |
| cluster-stateful | 200/s | 79.32 | `█████████████████` |
| cluster-stateless | 100/s | 5.65 | `███████` |
| cluster-stateless | 200/s | 5.9 | `███████` |
| cluster-stateless | 500/s | 5.85 | `███████` |
| cluster-stateless | 1000/s | 7.72 | `████████` |
| single-stateful | 100/s | 362.5 | `███████████████████████` |
| single-stateful | 200/s | 2212.55 | `██████████████████████████████` |
| single-stateless | 100/s | 5.73 | `███████` |
| single-stateless | 200/s | 5.43 | `██████` |
| single-stateless | 500/s | 8.49 | `████████` |
| single-stateless | 1000/s | 482.98 | `████████████████████████` |

## Notes

- **Target Rate**: The configured constant-arrival-rate (requests/second k6 attempts)
- **Actual Iter/s**: Achieved iterations per second (each iteration = 1 POST /api/exec)
- **HTTP Req/s**: Total HTTP requests per second (1 per iteration)
- **Dropped**: Iterations k6 couldn't schedule because VUs were exhausted (indicates server saturation)
- **Topology**: `single` = 1 MCP-V8 node; `cluster` = 3 MCP-V8 nodes with Raft

