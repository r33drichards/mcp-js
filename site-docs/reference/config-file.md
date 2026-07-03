# Configuration file

Complete reference for `--config`: configuring the entire server from a single
TOML or JSON file instead of (or in addition to) command-line flags.

## Flag

```
--config <PATH>
```

- Environment: `MCP_V8_CONFIG`
- Format is chosen by file extension: `.toml` or `.json`.

```bash
mcp-v8 --config /etc/mcp-v8/server.toml
# or
MCP_V8_CONFIG=/etc/mcp-v8/server.toml mcp-v8
```

## Precedence

Each setting is resolved independently, highest priority first:

1. Explicit command-line flag
2. `MCP_V8_*` environment variable
3. Config file
4. Built-in default

So a config file can hold the baseline and still be overridden ad hoc:
`mcp-v8 --config server.toml --http-port 9090` uses everything from
`server.toml` except the port.

## Keys

Every CLI flag is available as a key named after the flag; dashes and
underscores are interchangeable (`http-port` ≡ `http_port`). See the
[CLI flags](cli-flags.md) reference for the full list, value meanings, and
defaults — values in the config file are parsed exactly like flag values.

```toml
http_port = 8080
heap_store = "dir"                 # none | dir | s3
heap_dir = "/var/lib/mcp-v8/heaps"
heap_memory_max = 64               # MB
execution_timeout = 60             # seconds
allow_external_modules = true
instructions = "@/etc/mcp-v8/prompt.txt"   # @file works like on the CLI
peers = ["node2@10.0.0.2:4000", "10.0.0.3:4000"]  # repeatable flags take arrays
```

Rules enforced at startup (a violation is a fatal error, so typos cannot pass
silently):

- Unknown keys are rejected, and the error lists every accepted key.
- `http_port` and `sse_port` cannot both be set (same conflict as the flags).
- A key may appear only once (counting both spellings).
- Keys with no config-file meaning are rejected: `config` (no chain-loading)
  and `print_openapi`, plus `wasm_modules` / `wasm_stub_descriptions`, which
  are replaced by the `wasm` section below.

Relative paths in values are resolved against the server's working directory,
not the config file's location.

## Structured sections

Four sections hold structured data inline, replacing what is otherwise a
separate JSON file (or inline-JSON flag value). Each section is re-serialized
to JSON and handed to the corresponding flag's loader, so the schemas are
identical to the linked references.

| Section | Replaces | Shape | Schema reference |
|---|---|---|---|
| `wasm` | `--wasm-config` | table: name → path or object | [WebAssembly modules](wasm-modules.md) |
| `mcp_servers` | `--mcp-config` | array of server objects | [Calling upstream MCP servers](mcp-client.md) |
| `fetch_headers` | `--fetch-header-config` | array of rule objects | [Network access with fetch](fetch.md) |
| `policies` | `--policies-json` | object | [Security policies](policies.md) |

A section and its path-flag twin (e.g. `wasm` and `wasm_config`) cannot both
be set in the same file.

```toml
[wasm.math]
path = "/opt/modules/math.wasm"
max_memory_bytes = 16777216
description = "Adds two numbers and returns the sum"

[[mcp_servers]]
name = "weather"
transport = "stdio"
command = "python"
args = ["server.py"]

[[mcp_servers]]
name = "remote"
transport = "sse"
url = "http://localhost:9000/sse"

[[fetch_headers]]
host = "api.github.com"
methods = ["GET", "POST"]
headers = { Authorization = "Bearer ghp_..." }

[policies.fetch]
mode = "all"
policies = [{ url = "file:///etc/mcp-v8/fetch.rego" }]
```

## Complete example

A production-ish single file enabling HTTP transport, heap + filesystem
persistence on S3, hardening, and a policy-gated `fetch`:

```toml
# /etc/mcp-v8/server.toml
http_port = 8080
session_db_path = "/var/lib/mcp-v8/sessions"

heap_store = "s3"
fs_store = "s3"
s3_bucket = "my-mcp-v8-bucket"
cache_dir = "/var/cache/mcp-v8"

heap_memory_max = 64
execution_timeout = 60

harden_freeze_ops = true
harden_neutralize_proxy_details = true
harden_neutralize_introspection = true
harden_remove_bootstrap = true

[policies.fetch]
mode = "all"
policies = [{ url = "file:///etc/mcp-v8/fetch.rego" }]
```

The same document as JSON (`--config server.json`):

```json
{
  "http_port": 8080,
  "session_db_path": "/var/lib/mcp-v8/sessions",
  "heap_store": "s3",
  "fs_store": "s3",
  "s3_bucket": "my-mcp-v8-bucket",
  "cache_dir": "/var/cache/mcp-v8",
  "heap_memory_max": 64,
  "execution_timeout": 60,
  "harden_freeze_ops": true,
  "harden_neutralize_proxy_details": true,
  "harden_neutralize_introspection": true,
  "harden_remove_bootstrap": true,
  "policies": {
    "fetch": {
      "mode": "all",
      "policies": [{ "url": "file:///etc/mcp-v8/fetch.rego" }]
    }
  }
}
```
