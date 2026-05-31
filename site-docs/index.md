# mcp-v8

`mcp-v8` is a Rust-based MCP server that exposes a V8 JavaScript runtime to AI
agents and other clients. It supports stateful and stateless execution,
multiple transports, optional policy-gated network and filesystem access, and
content-addressed heap persistence.

`mcp-v8` is designed for cases where agents need real compute and controlled
access to host resources, but the cost of a full Linux VM is too high. It adds
a policy layer between the JavaScript runtime and the underlying machine so you
can expose network, filesystem, and other capabilities with tighter control and
lower overhead than VM or container-based approaches.

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

- how to [install](install/release-script.md) and [run the server](how-to/run-with-stdio.md)
- how [transports](concepts/transports.md) and [execution modes](concepts/execution-model.md) differ
- how [sessions](concepts/sessions-and-heaps.md), [heaps](concepts/sessions-and-heaps.md), and [clustering](concepts/clustering.md) work
- how policy-gated [fetch](concepts/network-access.md), [filesystem access](concepts/filesystem-access.md), and [module loading](concepts/module-loading.md) behave
- how to use the [HTTP API](reference/http-api.md)