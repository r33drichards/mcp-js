# Returning images & artifacts

Console output is text-only. Artifacts are how sandboxed code hands typed
payloads — images, audio, CSVs, arbitrary binary — back to the MCP client.
Images are the headline use case: the MCP spec defines an `ImageContent` tool
result block (base64 data + `mimeType`), and `image/*` artifacts are returned
as exactly that, so a model connected through MCP can *see* the image rather
than read about it.

## Store an artifact from JavaScript

Call the `artifact(key, mime, bytes)` global anywhere in your code:

```js
const png = renderChart();          // Uint8Array of PNG bytes
artifact("chart", "image/png", png);
```

- `key` — caller-chosen identifier (≤ 256 bytes). Writing the same key again
  overwrites it.
- `mime` — a `type/subtype` mime type, e.g. `image/png`, `text/csv`,
  `application/octet-stream`.
- `bytes` — a `Uint8Array`, any TypedArray, an `ArrayBuffer`, or a string
  (UTF-8 encoded). Max 16 MiB per artifact.

Invalid arguments throw a `TypeError`/`Error` you can catch in JS. Artifacts
are stored in the server's execution database, so they persist across
executions (and server restarts) and are shared across sessions — keys are a
single global namespace.

## Fetch an artifact over MCP

`get_artifact(key)` returns two content blocks: a JSON metadata block
(`key`, `mime_type`, `size_bytes`, `created_at`, `execution_id`, `encoding`)
followed by the payload rendered by mime type:

| Stored mime | MCP content block |
|-------------|-------------------|
| `image/*`   | `ImageContent` (base64 `data` + `mimeType` — the model sees the image) |
| `audio/*`   | `AudioContent` (base64 `data` + `mimeType`) |
| anything else, valid UTF-8 | `TextContent` with the raw text |
| anything else, binary | `TextContent` carrying base64 |

`list_artifacts()` returns metadata for everything stored.

## Discover what an execution produced

In async (stateful) mode, a completed execution lists what it emitted in the
`artifacts` field of `get_execution`:

```json
{ "tool": "get_execution", "arguments": { "execution_id": "01J9W…" } }
// Response: { "status": "completed", …,
//             "artifacts": [ { "key": "chart", "mime_type": "image/png",
//                              "size_bytes": 48213, … } ] }
```

Then fetch the payload:

```json
{ "tool": "get_artifact", "arguments": { "key": "chart" } }
```

In stateless mode, `run_js` attaches emitted artifacts directly to its own
tool result as content blocks (up to 8 MiB of payloads inline; larger
artifacts are listed in the result JSON with `"inline": false` and stay
retrievable via `get_artifact`).

## Fetch raw bytes over REST

The REST API serves artifact payloads verbatim — no base64 — with the stored
mime type as `Content-Type`:

```bash
curl http://localhost:8080/api/artifacts          # metadata list
curl http://localhost:8080/api/artifacts/chart -o chart.png
```

## Sizing images for models

Model providers cap image inputs (Claude, for example, rejects images over
~5 MB or 8000×8000 px, and tokens scale with pixel count), and base64 inflates
payloads by ~33%. Downscale or compress in JS before calling `artifact()` —
a chart rarely needs to be wider than ~1500 px.
