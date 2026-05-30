# Transport Walkthrough

`mcp-v8` supports three transport styles:

- stdio, which is the default when no network port is configured
- Streamable HTTP, enabled with `--http-port`
- SSE, enabled with `--sse-port`

## Stdio

Stdio is the simplest integration for local MCP clients that spawn the server
as a subprocess. It avoids network setup and is the default transport when no
HTTP or SSE port is provided.

## Streamable HTTP

Streamable HTTP is the best fit when you want a networked MCP endpoint and a
plain REST API from the same process. In this mode, MCP is served at `/mcp`,
and the REST API remains available under `/api/...`.

## SSE

SSE is the older HTTP-based transport. It exposes `/sse` for the event stream
and `/message` for client requests. Use it when your client requires SSE
instead of the newer Streamable HTTP model.

## Choosing between them

- use stdio for local tool integrations
- use Streamable HTTP for most server deployments
- use SSE only when the client requires it

For the transport model behind these setup choices, see
[Transports](../concepts/transports.md).
