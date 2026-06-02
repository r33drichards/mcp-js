# MCP Protocol Integration

mcp-v8 implements the Model Context Protocol (MCP) using the `rmcp` Rust crate. MCP is the standard that AI agents use to discover and invoke tools, read resources, and maintain sessions with external services. mcp-v8 acts as an MCP server, exposing JavaScript execution and related capabilities as MCP tools.

## How Tool Registration Works

MCP tools are defined using rmcp's `#[tool]` attribute macro on handler methods. mcp-v8 has two MCP service implementations:

- **McpService** -- Used in stateful mode. Registers tools for executing code with heap snapshots, session management, heap tagging, and execution lifecycle.
- **StatelessMcpService** -- Used in stateless mode. Registers a subset of tools (no session or heap-related tools).

Each tool has a name, a description, and a JSON Schema for its input parameters. When an MCP client calls `tools/list`, rmcp returns the list of registered tools with their schemas. When a client calls `tools/call`, rmcp dispatches to the corresponding Rust handler method.

The core tools available in both modes include:

- `run_js` -- Submit JavaScript/TypeScript code for execution. Returns an execution ID.
- `get_execution_status` -- Check whether an execution is running, completed, failed, cancelled, or timed out.
- `get_execution_output` -- Retrieve paginated console output from an execution.
- `cancel_execution` -- Terminate a running execution.

Stateful mode adds:

- `list_sessions` -- List all named sessions.
- `list_session_snapshots` -- List the execution history for a session.
- `get_heap_tags` / `set_heap_tags` / `delete_heap_tags` / `query_heaps_by_tags` -- Manage metadata on heap snapshots.

## Resource Serving

MCP resources are read-only documents that clients can request. mcp-v8 serves four documentation resources:

| URI | Content |
|---|---|
| `docs://readme` | Full README in Markdown |
| `docs://llms-txt` | Machine-readable agent guide following the llms.txt convention |
| `docs://openapi` | OpenAPI 3.0 JSON specification for the REST API |
| `docs://tools` | JSON catalog of available MCP tools with descriptions and schemas |

These resources let an AI agent discover what mcp-v8 can do without external documentation. An agent can read `docs://llms-txt` to understand the available tools and their parameters, or read `docs://openapi` to understand the REST API.

Resources are served through the standard MCP `resources/list` and `resources/read` methods. The content is generated at runtime (OpenAPI spec and tool catalog reflect the current server configuration).

## X-MCP-Session-Id Header

When using HTTP-based transports (Streamable HTTP or SSE), MCP sessions are identified by the `X-MCP-Session-Id` header. The lifecycle is:

1. The client sends an `initialize` request without a session ID.
2. The server generates a new session ID and returns it in the response header.
3. All subsequent requests from the client include this session ID.

In mcp-v8, this header serves two purposes:

- **Session routing** -- In stateful mode, the session ID can be passed as the `session` parameter to `run_js`, linking executions to a named session log.
- **JWT verification** -- When `--jwks-url` is configured, the server verifies the JWT in the `Authorization: Bearer` header during the `initialize` handshake. The session ID is then trusted for subsequent requests.

## MCP Tool Stubs (Pass-Through)

When mcp-v8 is connected to upstream MCP servers (via `--mcp-server` or `--mcp-config`), it can optionally expose those upstream tools as stub tools on its own MCP interface. Each upstream tool is re-registered with a configurable prefix (default: `runjs__`).

When a client calls a stub tool, mcp-v8 generates JavaScript code that calls `mcp.callTool()` with the appropriate server, tool name, and arguments, then executes that code through the normal `run_js` path. This makes upstream MCP tools accessible to clients that connect only to mcp-v8, without those clients needing direct connections to the upstream servers.

## Server Capabilities

During the MCP `initialize` handshake, mcp-v8 advertises its capabilities:

- **Tools** -- Always advertised. The tool list reflects the current mode (stateful or stateless) and any upstream MCP tool stubs.
- **Resources** -- Always advertised. The four documentation resources are always available.

The server info includes the name "mcp-v8" and the version from the Cargo package metadata.
