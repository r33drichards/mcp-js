# Overview

`mcp-v8` is a JavaScript execution server built around V8 and exposed through
the Model Context Protocol. It is designed for clients that need to run
JavaScript or TypeScript, optionally carry heap state between runs, and inspect
results through MCP tools or the HTTP API.

At a high level, the system has four layers:

1. transport entry points such as stdio, Streamable HTTP, and SSE
2. an execution engine that runs JavaScript in V8
3. optional persistence for heaps and session history
4. optional policy-gated capabilities such as `fetch()`, `fs`, and external
   module loading

If you are new to the project, continue with [First Run](first-run.md). If you
already know what transport or client you need, move to
[Transport Walkthrough](transport-walkthrough.md) or
[Client Walkthrough](client-walkthrough.md).
