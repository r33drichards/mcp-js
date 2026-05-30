# mcp-v8

`mcp-v8` is a Rust-based MCP server that exposes a V8 JavaScript runtime to AI
agents and other clients. It supports stateful and stateless execution,
multiple transports, optional policy-gated network and filesystem access, and
content-addressed heap persistence.

## Quick Start

If you want the fastest path to a working setup, start with
[Quick Start](quick-start/overview.md). It includes entry points for Claude Code,
Codex, Cursor, generic MCP clients, `curl`, and the bundled CLI.

Use this documentation by intent:

- Start with [Quick Start](quick-start/overview.md) if you need orientation.
- Use [How-to](how-to/overview.md) when you need to complete a task.
- Read [Concepts](concepts/overview.md) to understand system behavior.
- Use [Reference](reference/overview.md) for flags, APIs, and interface details.

## What this site covers

- how to install and run the server
- how transports and execution modes differ
- how sessions, heaps, and clustering work
- how policy-gated fetch, filesystem access, and module loading behave
- how to use the HTTP API and Rust client

## Core concepts

- [Execution Model](concepts/execution-model.md)
- [Sessions and Heaps](concepts/sessions-and-heaps.md)
- [Integration Surfaces](concepts/integration-surfaces.md)
- [Module Loading](concepts/module-loading.md)
- [WASM and Native Modules](concepts/wasm-and-native-modules.md)
- [Policy System](concepts/policy-system.md)
- [Network Access](concepts/network-access.md)
- [Filesystem Access](concepts/filesystem-access.md)
- [Clustering](concepts/clustering.md)
