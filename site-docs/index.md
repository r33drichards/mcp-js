# mcp-v8

A Rust-based MCP (Model Context Protocol) server that runs JavaScript and TypeScript inside a V8 engine. Designed for AI agents that need a sandboxed, stateful JS runtime as a tool.

mcp-v8 exposes a V8 JavaScript runtime as MCP tools. Agents can run JS/TS code, persist V8 heap state between calls (stateful mode), import npm/JSR packages at runtime, and optionally use OPA-gated fetch and filesystem access.

## Documentation Structure

This documentation follows the [Diataxis](https://diataxis.fr/) framework, organized into four pillars:

### [Tutorials](tutorials/overview.md)

**Learning-oriented** step-by-step lessons. Start here if you're new to mcp-v8. Each tutorial walks you through a complete working example from start to finish.

### [How-to Guides](how-to/overview.md)

**Task-oriented** practical guides. Use these when you know what you want to do and need the steps to get it done. Each guide focuses on a single task.

### [Concepts](concepts/overview.md)

**Understanding-oriented** explanations. Read these to build a deeper mental model of how mcp-v8 works and why it's designed the way it is.

### [Reference](reference/overview.md)

**Information-oriented** technical specifications. Look up CLI flags, API endpoints, tool schemas, and configuration formats.

## Quick Links

| What you want to do | Where to go |
|---|---|
| Install mcp-v8 | [Installation tutorial](tutorials/installation.md) |
| Run your first code | [JavaScript runtime tutorial](tutorials/javascript-runtime.md) |
| Connect to Claude or Cursor | [MCP protocol tutorial](tutorials/mcp-protocol.md) |
| Look up a CLI flag | [CLI flags reference](reference/cli-flags.md) |
| Understand stateful vs stateless | [Execution model concepts](concepts/execution-model.md) |
| Deploy with Docker | [Docker deployment how-to](how-to/docker-deployment.md) |

## Source

- GitHub: [r33drichards/mcp-js](https://github.com/r33drichards/mcp-js)
- Install: `curl -fsSL https://raw.githubusercontent.com/r33drichards/mcp-js/main/install.sh | sudo bash`
