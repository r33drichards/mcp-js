# @mcp-v8/client (TypeScript)

A TypeScript client for the [mcp-v8](https://github.com/r33drichards/mcp-js)
JavaScript execution server. The wire types are **generated from the server's
OpenAPI spec** (`openapi.json` at the repo root) with
[`openapi-typescript`](https://openapi-ts.dev/), and requests go through the
tiny, fully-typed [`openapi-fetch`](https://openapi-ts.dev/openapi-fetch/)
runtime — mirroring how the Rust client (`mcp-v8-client`) is generated with
progenitor from the same spec.

## Install

```bash
npm install   # from clients/typescript
```

## Regenerating the types

Whenever the server's HTTP API changes, regenerate `openapi.json` (e.g.
`./server --print-openapi > openapi.json`) and then:

```bash
npm run generate   # openapi-typescript ../../openapi.json -> src/schema.d.ts
```

`src/schema.d.ts` is generated and checked in; do not edit it by hand.

## Usage

```ts
import { createMcpV8Client } from "@mcp-v8/client";

const client = createMcpV8Client("http://localhost:8080");

// One-shot: submit, wait for completion, collect console output.
const { status, output, error, heap } = await client.runJs(
  "console.log('hello'); 1 + 1",
);
console.log(status, output); // "completed", "hello\n"

// Stateful: thread the returned heap key into the next call.
const next = await client.runJs("globalThis.x = (globalThis.x ?? 0) + 1; console.log(x)", {
  heap,
});
```

### Low-level REST access

`runJs` is a convenience over the raw endpoints, which are also exposed:

```ts
const { execution_id } = await client.exec({ code: "console.log(1)" });
const info = await client.getExecution(execution_id);
const page = await client.getExecutionOutput(execution_id, { line_offset: 0 });
await client.cancelExecution(execution_id);
```

All request/response types are re-exported (`ExecRequest`, `ExecutionInfo`,
`ExecutionOutput`, etc.) and derived from the generated schema.
