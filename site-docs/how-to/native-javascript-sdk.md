# Embed the native JavaScript SDK

Use `@wholelottahoopla/mcp-js-node` to execute guest JavaScript inside your Node.js
application. It loads the native engine through UniFFI; the separate
`@wholelottahoopla/mcp-js-client` package is an HTTP client for a running server.

## Install the package for your host

Choose a published GitHub release containing SDK assets and replace `VERSION`
with its version, including a prerelease suffix when applicable:

```bash
npm install '@wholelottahoopla/mcp-js-node@https://github.com/r33drichards/mcp-js/releases/download/vVERSION/wholelottahoopla-mcp-js-node-VERSION.tar.gz'
```

The standard native asset targets Linux x64 with glibc 2.39 or newer. For Alpine
x64, select a release containing the musl asset explicitly:

```bash
bun add '@wholelottahoopla/mcp-js-node@https://github.com/r33drichards/mcp-js/releases/download/vVERSION/wholelottahoopla-mcp-js-node-VERSION-linux-x64-musl.tar.gz'
```

The Node package targets Node.js 22+. The musl packaging check also exercises
Node and Bun in Alpine. These assets do not establish native macOS, Windows,
ARM64, iOS, or Android SDK support. Server-binary platforms are a separate matrix.

## Run code and dispose the engine

```javascript
import { Engine } from '@wholelottahoopla/mcp-js-node';

const engine = Engine.createStateless(64n, 5n);
try {
  const result = JSON.parse(await engine.callToolAsync(
    'run_js', JSON.stringify({ code: 'console.log(6 * 7)' }),
    undefined, undefined,
  ));
  console.log(result.output);
} finally {
  engine.dispose();
}
```

`callToolAsync` returns a Promise without blocking the Node event loop.
`callTool` is synchronous and blocks the caller. Both use the engine's normal
limits and error/result contracts. Explicit disposal shuts down the engine and
releases its native handle; repeating it is safe.

## Supply host tools to guest code

```javascript
import { Engine, createToolRuntime } from '@wholelottahoopla/mcp-js-node';

const engine = Engine.createStateless(64n, 5n);
try {
  const runtime = createToolRuntime(engine, {
    server: 'host',
    tools: {
      echo: {
        description: 'Echo a message',
        inputSchema: {
          type: 'object',
          properties: { message: { type: 'string' } },
          required: ['message'],
        },
        run({ message }) {
          if (typeof message !== 'string') throw new Error('Expected a string');
          return { message };
        },
      },
    },
  });
  const result = await runtime.runJs(`
    const reply = await mcp.callTool('host', 'echo', { message: 'hello' });
    console.log(JSON.parse(reply.content[0].text).message);
  `);
  console.log(result.output);
} finally {
  engine.dispose();
}
```

The wrapper allocates a separate request channel per run, with at most 32 host
calls and 256 KiB request/response JSON limits. Validate inputs and enforce
application permissions in the handler. A handler receives an `AbortSignal`;
`runtime.runJs(code, signal)` supports cancellation. An abort does not undo an
external effect already performed by a handler.

`runtime.listTools()` returns discovery stubs. The wrapper supports engines
without heap snapshots or configured upstream MCP servers. To add the same
MCP authorization/hooks as the server, use
`EngineConfigBuilder.mcpToolsPoliciesJson(...)`; see the
[complete SDK configuration](https://github.com/r33drichards/mcp-js/blob/main/node/README.md).

## Build and validate a package

Use the current [SDK build recipe](https://github.com/r33drichards/mcp-js/blob/main/node/README.md#build-and-validate).
The glibc build uses the default Nix shell's pinned `RUSTY_V8_ARCHIVE`; the
published V8 crate lacks ICU inputs needed by the old source-rebuild recipe.
The musl recipe is `node/scripts/package-musl.sh`. Packaging validates ELF
architecture/dependencies, rejects Nix-store RPATHs, installs outside the source
checkout, and runs real guest code and host tools. Do not bypass those checks.

## Source

- [Host wrapper](https://github.com/r33drichards/mcp-js/blob/main/node/src/tool-runtime.ts)
- [Engine disposal](https://github.com/r33drichards/mcp-js/blob/main/node/src/engine-dispose.ts)
- [Host channel](https://github.com/r33drichards/mcp-js/blob/main/server/src/engine/host_tools.rs)
- [Consumer tests](https://github.com/r33drichards/mcp-js/blob/main/node/tests/consumer.mjs)
