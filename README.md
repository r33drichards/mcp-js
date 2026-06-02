# mcp-v8: V8 JavaScript MCP Server

A Rust-based Model Context Protocol (MCP) server that exposes a V8 JavaScript runtime as a tool for AI agents like Claude and Cursor. Supports persistent heap snapshots via S3 or local filesystem, and is ready for integration with modern AI development environments.

## Documentation

Full documentation is available at the [mcp-v8 docs site](https://r33drichards.github.io/mcp-js/), organized into four pillars:

- **[Tutorials](https://r33drichards.github.io/mcp-js/tutorials/overview/)** - Step-by-step learning guides
- **[How-to Guides](https://r33drichards.github.io/mcp-js/how-to/overview/)** - Task-oriented practical guides
- **[Concepts](https://r33drichards.github.io/mcp-js/concepts/overview/)** - Understanding-oriented explanations
- **[Reference](https://r33drichards.github.io/mcp-js/reference/overview/)** - Technical specifications

## Quick Start

Install mcp-v8:

```bash
curl -fsSL https://raw.githubusercontent.com/r33drichards/mcp-js/main/install.sh | sudo bash
```

Run in stateless mode with HTTP transport:

```bash
mcp-v8 --stateless --http-port 3000
```

Run in stateful mode with local storage:

```bash
mcp-v8 --directory-path /tmp/mcp-v8-heaps --http-port 3000
```

Connect from Claude Code:

```bash
claude mcp add mcp-v8 -- mcp-v8 --stateless
```

## Features

- **V8 JavaScript/TypeScript Execution** - Run JS/TS in a secure, isolated V8 engine
- **Stateful Heap Snapshots** - Persist and resume V8 state between executions via content-addressed storage
- **Multiple Transports** - stdio, Streamable HTTP, and SSE
- **ES Module Imports** - Import npm, JSR, and URL modules at runtime via esm.sh
- **WebAssembly** - Compile and run WASM modules, including pre-loaded globals
- **OPA Policy System** - Policy-gated fetch, filesystem, modules, subprocess, and MCP tools
- **Fetch Header Injection** - Static headers and OAuth2 client-credentials token injection
- **Raft Clustering** - Multi-node distributed coordination and replicated session logging
- **HTTP REST API** - Plain REST endpoints alongside MCP transport
- **CLI Client** - Fully-typed `mcp-v8-cli` generated from OpenAPI spec
- **MCP Pass-Through** - Connect upstream MCP servers and call their tools from JavaScript
- **Docker Ready** - Multi-stage Dockerfile and docker-compose configurations

## License

See [LICENSE](LICENSE).
