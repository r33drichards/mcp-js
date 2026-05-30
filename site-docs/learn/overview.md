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

## Read next by topic

- Read [Integration Surfaces](../concepts/integration-surfaces.md) to
  understand the MCP-first model and the fallback HTTP and CLI paths.
- Read [Execution Model](../concepts/execution-model.md) to understand the
  async lifecycle and output model.
- Read [Sessions and Heaps](../concepts/sessions-and-heaps.md) to understand
  stateful versus stateless behavior.
- Read [Module Loading](../concepts/module-loading.md) if your code imports
  npm, JSR, or URL modules.
- Read [Policy System](../concepts/policy-system.md) if you plan to enable
  network or filesystem capabilities.
