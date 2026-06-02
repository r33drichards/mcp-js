# CLI Client Architecture

mcp-v8 includes a command-line client (`mcp-v8-cli`) that provides a terminal interface to a running mcp-v8 server. The CLI is auto-generated from the OpenAPI specification, ensuring that it always matches the server's API surface.

## Auto-Generation from OpenAPI

The CLI client is built using the `progenitor` crate, which takes an OpenAPI specification and generates a complete Rust HTTP client library. The build process is:

1. The server's OpenAPI spec is captured as `openapi.json` in the `mcp-v8-client` crate.
2. At compile time, `progenitor` reads this spec and generates typed Rust functions for every endpoint.
3. The CLI binary wraps these generated functions with a clap-based command-line interface.

This approach has two advantages: the CLI never falls out of sync with the server API, and adding a new endpoint to the server automatically makes it available in the CLI after regeneration.

## Commands

The CLI mirrors the REST API endpoints:

- **exec** -- Submit code for execution. Accepts code as an argument or reads from stdin.
- **status** -- Check the status of an execution by ID.
- **output** -- Retrieve console output from an execution, with pagination options.
- **cancel** -- Cancel a running execution.
- **list** -- List all executions.
- **sessions** -- List sessions (stateful mode).
- **session-snapshots** -- List entries in a session log.
- **heap-tags** -- Get, set, or delete tags on heap snapshots.
- **query-heaps** -- Query heaps by tag filter.

## JSON Output Mode

The CLI supports a `--json` flag (or `MCP_V8_JSON=1` environment variable) that outputs raw JSON responses instead of formatted text. This makes the CLI suitable for scripting and piping to tools like `jq`:

```bash
mcp-v8-cli exec "1 + 1" --json | jq .execution_id
```

## MCP_V8_URL Environment Variable

The CLI connects to a server via the `MCP_V8_URL` environment variable:

```bash
export MCP_V8_URL=http://localhost:3000
mcp-v8-cli exec "console.log('hello')"
```

This can also be passed as a `--url` flag. The URL should point to the server's HTTP port (the same port used for the REST API and MCP Streamable HTTP transport).

## CLI Distribution

The CLI binary can be downloaded directly from a running mcp-v8 server:

- `GET /api/cli` returns an index of available platform binaries.
- `GET /api/cli/<platform>` downloads the binary for a specific platform (linux-x86_64, linux-aarch64, macos-aarch64).

CLI binaries are embedded in the server binary at compile time (when the corresponding `MCP_V8_CLI_*` environment variables point to the built CLI binaries). The `install-cli.sh` script automates the download-and-install process.

## Design Rationale

The auto-generation approach was chosen over a hand-written CLI for maintenance reasons. The mcp-v8 API surface is non-trivial (executions, sessions, tags, output pagination), and keeping a hand-written CLI synchronized with every API change is error-prone. By deriving the CLI from the same OpenAPI spec that documents the API, the two are guaranteed to agree.

The trade-off is that the generated client is sometimes more verbose than a hand-crafted one, and the CLI's command structure mirrors the API's URL structure rather than being independently optimized for ergonomics. In practice, this has been an acceptable trade-off because the primary users of the CLI are developers and scripts, not end users.
