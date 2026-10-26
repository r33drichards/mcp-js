# Create your first HTTP function

In this lesson you will register a JavaScript handler, call it, and remove it.
You need this management-layer checkout, Rust, Python 3, curl, and two terminals.
See [feature status](../reference/scheduler-http-status.md) for the exact baseline.
All requests stay on loopback. This lesson uses an unauthenticated local server;
use [authentication](../how-to/authentication.md) for a shared deployment.

## Start the server

From the repository root in terminal 1:

```sh
cargo build --locked -p server --bin server
export MCP_V8_HTTP_FUNCTIONS_DIR="$(mktemp -d)"
./target/debug/server --heap-store none --http-port 8080 --bind-host 127.0.0.1
```

Keep the server running. Its fresh registry directory belongs to this lesson.

## Register a greeting

In terminal 2, write a definition to a temporary file:

```sh
cat > /tmp/mcp-greeting.json <<'JSON'
{
  "version": 1,
  "methods": ["GET"],
  "export": "handler",
  "source": "function handler(request) { return {status: 200, headers: {'content-type': 'text/plain'}, body: 'Hello from ' + request.path}; }",
  "limits": {"timeout_seconds": 5, "heap_mb": 32}
}
JSON
curl --fail-with-body -i -X PUT   http://127.0.0.1:8080/api/http-functions/v1/definitions/greeting   -H 'Content-Type: application/json' -H 'If-None-Match: *'   --data-binary @/tmp/mcp-greeting.json
```

You should receive a successful response with a quoted `ETag`. Registration
stores the definition; it does not execute the handler.

## Call the handler

```sh
curl --fail-with-body http://127.0.0.1:8080/functions/v1/greeting
```

The response body is `Hello from /functions/v1/greeting`. The handler returns
its response object directly; `console.log` would produce execution logs instead.
A POST to this GET-only definition returns 405.

## Invoke the same function through MCP

Keep the server running. In terminal 2, send a sessionless MCP call:

```sh
curl --fail-with-body http://127.0.0.1:8080/mcp \
  -H 'Content-Type: application/json' \
  -H 'Accept: application/json, text/event-stream' \
  -H 'MCP-Protocol-Version: 2026-07-28' \
  -H 'Mcp-Method: tools/call' \
  -H 'Mcp-Name: http_function_invoke' \
  --data-binary '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"http_function_invoke","arguments":{"name":"greeting","method":"GET"},"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientInfo":{"name":"management-lesson","version":"1"},"io.modelcontextprotocol/clientCapabilities":{}}}}'
```

The MCP result contains a JSON content block with `status: 200`, response headers,
and a base64 body. Decode `body_base64` to see the same greeting returned by HTTP.
Both calls used the definition you registered above. MCP does not require a
second registration or a separate registry directory.

## Inspect and remove it

```sh
curl --fail-with-body -i   http://127.0.0.1:8080/api/http-functions/v1/definitions/greeting
```

Copy the quoted ETag into the next request, replacing `REVISION` and keeping the
quotes inside the header value:

```sh
curl --fail-with-body -i -X DELETE   http://127.0.0.1:8080/api/http-functions/v1/definitions/greeting   -H 'If-Match: "REVISION"'
curl -i http://127.0.0.1:8080/functions/v1/greeting
```

The final request returns 404. Stop terminal 1 with Ctrl-C. You have learned the
registration, invocation, and conditional deletion cycle. Next, learn to
[update handlers safely](../how-to/manage-http-functions.md) or consult the
[request and response contract](../reference/http-functions.md).
