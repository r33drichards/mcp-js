# Tutorials

Welcome to the mcp-v8 tutorials. These step-by-step lessons walk you through real tasks from start to finish, building your understanding of mcp-v8 as you go.

## Who are these for?

These tutorials are for anyone getting started with mcp-v8 -- whether you are an AI agent developer, a platform engineer, or just curious about running JavaScript inside an MCP server. No prior MCP experience is required, though basic familiarity with JavaScript and the command line is assumed.

## How to use these tutorials

Each tutorial is self-contained and focuses on a single feature or concept. They are ordered roughly from foundational to advanced:

1. **[Installation](installation.md)** -- Install mcp-v8 and the CLI client
2. **[Running Your First JS/TS Code](javascript-runtime.md)** -- Execute JavaScript and TypeScript in the V8 engine
3. **[Understanding Stateful vs Stateless Execution](execution-model.md)** -- Compare the two execution modes
4. **[Building a Stateful Session](sessions-and-heaps.md)** -- Work with heap snapshots and session history
5. **[Working with Console Output](console-output.md)** -- Read logs with pagination and different modes
6. **[Organizing Heap Snapshots with Tags](heap-tags.md)** -- Label and query your sessions
7. **[Connecting via Different Transports](transports.md)** -- Set up stdio, HTTP, and SSE
8. **[Integrating with AI Clients](mcp-protocol.md)** -- Connect to Claude Desktop, Claude Code, Cursor, and Codex
9. **[Importing External Packages](module-loading.md)** -- Use npm, JSR, and URL imports
10. **[Running WebAssembly](webassembly.md)** -- Execute WASM modules inside V8
11. **[Setting Up OPA Policies](policy-system.md)** -- Control what code can do with Rego policies
12. **[Making HTTP Requests](network-access.md)** -- Use the Fetch API with policy controls
13. **[Automatic Credential Injection](fetch-header-injection.md)** -- Inject headers and OAuth2 tokens
14. **[Reading and Writing Files](filesystem-access.md)** -- Access the filesystem with policy controls
15. **[Setting Up a Multi-Node Cluster](clustering.md)** -- Build a Raft-based cluster with failover
16. **[Configuring Heap Storage](storage-backends.md)** -- Use local, S3, or write-through storage
17. **[Using the REST API Directly](http-api.md)** -- Call mcp-v8 with curl
18. **[Using the CLI Client](mcp-v8-cli.md)** -- Interact with mcp-v8 from the terminal
19. **[Connecting Upstream MCP Servers](mcp-pass-through.md)** -- Bridge external MCP servers into JS
20. **[Deploying with Docker](docker-deployment.md)** -- Containerize single nodes and clusters
21. **[Installation](installation.md)** -- All installation methods

Start with [Installation](installation.md) if you have not yet installed mcp-v8, or jump to [Running Your First JS/TS Code](javascript-runtime.md) if you are ready to go.
