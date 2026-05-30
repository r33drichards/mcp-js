# Client Walkthrough

There are three common ways to interact with `mcp-v8`:

- through MCP tools exposed over stdio, HTTP, or SSE
- through the plain HTTP API
- through the generated Rust client and CLI

## MCP clients

In MCP mode, the main entry point is the `run_js` tool. In stateful mode,
`run_js` queues background work and returns an execution ID. In stateless mode,
it waits internally and returns output directly.

Stateful mode also exposes status, output, cancellation, session, and heap-tag
tools. The exact tool shape depends on how the server was started.

## HTTP clients

When running in HTTP or SSE mode, the server also exposes a REST API. That API
mirrors the async execution flow used by the stateful MCP tool surface:
submit, poll, inspect output, and optionally cancel.

## Rust client and CLI

The repository ships a generated Rust SDK and a CLI that both target the HTTP
API. They are a good fit when you want typed automation outside MCP.
