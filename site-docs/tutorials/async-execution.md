# Asynchronous execution & output

In this tutorial we'll submit a JavaScript snippet, poll until it finishes, and read its console output — all using the REST API over an HTTP transport.

## Prerequisites

- mcp-v8 running with `--http-port=8080` (or `--sse-port=8080`).
- `curl` and `jq` available on your path.

Start the server if it isn't already running:

```bash
mcp-v8 --http-port=8080
```

## Step 1 — Submit code for execution

POST to `/api/exec` with a JSON body. The only required field is `code`.

```bash
curl -s -X POST http://localhost:8080/api/exec \
  -H 'Content-Type: application/json' \
  -d '{
    "code": "for (let i = 1; i <= 5; i++) console.log(\"line\", i);"
  }'
```

The server returns `202 Accepted` immediately with an execution ID:

```json
{ "execution_id": "01J9W4KXNE000000000000001" }
```

Save that ID — we'll use it in the next steps.

```bash
EID="01J9W4KXNE000000000000001"
```

## Step 2 — Poll for completion

Call `GET /api/executions/{id}` in a loop until `status` is no longer `running`.

```bash
while true; do
  STATUS=$(curl -s http://localhost:8080/api/executions/$EID | jq -r .status)
  echo "status: $STATUS"
  [ "$STATUS" != "running" ] && break
  sleep 0.2
done
```

When finished the response body includes the full record:

```json
{
  "execution_id": "01J9W4KXNE000000000000001",
  "status": "completed",
  "result": null,
  "heap": "sha256:abc123…",
  "error": null,
  "started_at": "2024-11-01T12:00:00.000Z",
  "completed_at": "2024-11-01T12:00:00.023Z"
}
```

Possible values for `status`: `running`, `completed`, `failed`, `cancelled`, `timed_out`.

## Step 3 — Read console output

Fetch the first page of output (default: up to 100 lines):

```bash
curl -s "http://localhost:8080/api/executions/$EID/output" | jq .
```

```json
{
  "execution_id": "01J9W4KXNE000000000000001",
  "data": "line 1\nline 2\nline 3\nline 4\nline 5",
  "start_line": 1,
  "end_line": 5,
  "next_line_offset": 6,
  "total_lines": 5,
  "start_byte": 0,
  "end_byte": 35,
  "next_byte_offset": 35,
  "total_bytes": 35,
  "has_more": false,
  "status": "completed"
}
```

`has_more: false` means we got everything in one page. When `has_more` is `true`, pass `next_line_offset` as `line_offset` to fetch the next page.

## Step 4 — Page through long output

For scripts that produce many lines, iterate using `line_offset` and `line_limit`:

```bash
OFFSET=1
while true; do
  PAGE=$(curl -s "http://localhost:8080/api/executions/$EID/output?line_offset=$OFFSET&line_limit=50")
  echo "$PAGE" | jq -r .data
  HAS_MORE=$(echo "$PAGE" | jq -r .has_more)
  [ "$HAS_MORE" != "true" ] && break
  OFFSET=$(echo "$PAGE" | jq -r .next_line_offset)
done
```

## Step 5 — Check the execution list

To see all executions the server currently knows about:

```bash
curl -s http://localhost:8080/api/executions | jq .
```

```json
{
  "executions": [
    {
      "execution_id": "01J9W4KXNE000000000000001",
      "status": "completed",
      "started_at": "2024-11-01T12:00:00.000Z",
      "completed_at": "2024-11-01T12:00:00.023Z"
    }
  ]
}
```

## What we covered

- POST `/api/exec` accepts code and returns an `execution_id` immediately.
- GET `/api/executions/{id}` reports status and result.
- GET `/api/executions/{id}/output` serves paginated console output with cursor fields for iteration.
- The execution list gives an overview of all tracked runs.

## See also

- [Concepts: Asynchronous execution & output](../concepts/async-execution.md)
- [How-to: Asynchronous execution & output](../how-to/async-execution.md)
- [Reference: Asynchronous execution & output](../reference/async-execution.md)
- [Quick-start: Running JavaScript & TypeScript](js-execution.md)
- [Reference: HTTP API](../reference/http-api.md)
- [Concepts: Transports](../concepts/transports.md)
