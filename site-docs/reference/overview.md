# Reference

Complete, factual reference material: flags, tool parameters, JS APIs, endpoints, and
defaults. For learning and tasks, see the [Tutorials](../tutorials/overview.md)
and [How-to guides](../how-to/overview.md).

## Generated references

These pages are generated from the source of truth and must not be edited by hand:

- [CLI flags](cli-flags.md) — every command-line flag (from the Clap definition).
- [HTTP API](http-api.md) — the REST surface (from `openapi.json` via Widdershins).
- [MCP tools](mcp-tools.md) — the built-in MCP tools (from the tool registry).

## Reference by feature

- [Running JavaScript & TypeScript](js-execution.md) — `run_js` params, return shapes, limits.
- [Stateful sessions & heap snapshots](sessions-and-heaps.md) — session/heap tools, log fields, key format.
- [Heap storage backends](storage-backends.md) — storage flags, defaults, exclusivity.
- [Asynchronous execution & output](async-execution.md) — async tools, status enum, pagination fields.
- [Transports: stdio, HTTP, SSE](transports.md) — flags, endpoint paths, keep-alive.
- [Network access with fetch](fetch.md) — `fetch()` API, header/OAuth injection schemas.
- [Filesystem access](filesystem.md) — every `fs.*` method and policy input.
- [Subprocess execution](subprocess.md) — subprocess API and policy input.
- [WebAssembly modules](wasm-modules.md) — module flags, config schema, JS global shape.
- [ES module imports](module-imports.md) — flag, policy entrypoint and input.
- [Calling upstream MCP servers](mcp-client.md) — `--mcp-server` formats, `mcp.*` API.
- [Security policies (OPA/Rego)](policies.md) — `--policies-json` schema and per-category inputs.
- [Authentication (JWT/JWKS)](authentication.md) — JWKS flag, accepted headers, verification.
- [Clustering & replication (Raft)](clustering.md) — cluster flags, peer format, Raft endpoints.
