# HTTP API

> Generated from the repo root `openapi.json`. Do not edit this page by hand.

When `mcp-v8` runs with `--http-port` or `--sse-port`, it exposes a plain HTTP
API alongside MCP transport.

OpenAPI version: `3.0.3`

API title: `mcp-v8`

API version: `0.1.0`

## Endpoints

| Method | Path | Summary |
| --- | --- | --- |
| `GET` | `/api/cli` | List available CLI binary downloads for the running server version. |
| `GET` | `/api/cli/{platform}` | Download the CLI binary for a specific platform directly from this server. |
| `POST` | `/api/exec` | Submit JavaScript code for asynchronous execution. |
| `GET` | `/api/executions` | List all known executions (running and recently completed). |
| `GET` | `/api/executions/{id}` | Get the status and result of an execution. |
| `POST` | `/api/executions/{id}/cancel` | Cancel a running execution. |
| `GET` | `/api/executions/{id}/output` | Read paginated console output from an execution. |
| `GET` | `/api/version` | Return the running server version. |

## `GET /api/cli`

List available CLI binary downloads for the running server version.

Each `url` is a direct download from this server. `available: false` means the binary was not embedded at build time (dev/local builds).

Tags: `cli`

### Responses

| Status | Content Type | Schema | Description |
| --- | --- | --- | --- |
| `200` | `application/json` | [`CliIndex`](#schema-cliindex) | CLI download index |

## `GET /api/cli/{platform}`

Download the CLI binary for a specific platform directly from this server.

The binary is embedded at build time and always matches the running server version. Returns 404 if the server was built without embedded binaries (dev/local builds).  Supported platforms: `linux-x86_64`, `linux-aarch64`, `macos-aarch64`.

Tags: `cli`

### Parameters

| Name | In | Required | Type | Description |
| --- | --- | --- | --- | --- |
| `platform` | `path` | yes | string | Target platform (linux-x86_64 | linux-aarch64 | macos-aarch64) |

### Responses

| Status | Content Type | Schema | Description |
| --- | --- | --- | --- |
| `200` | - | - | CLI binary (application/octet-stream) |
| `404` | `application/json` | [`ApiError`](#schema-apierror) | Unknown platform or binary not embedded |

## `POST /api/exec`

Submit JavaScript code for asynchronous execution.

Returns immediately with an `execution_id`. Use `GET /api/executions/{id}` to poll status and `GET /api/executions/{id}/output` to read console output.

Tags: `executions`

### Request Body

Required.

| Content Type | Schema |
| --- | --- |
| `application/json` | [`ExecRequest`](#schema-execrequest) |

### Responses

| Status | Content Type | Schema | Description |
| --- | --- | --- | --- |
| `202` | `application/json` | [`ExecAccepted`](#schema-execaccepted) | Execution queued |
| `500` | `application/json` | [`ApiError`](#schema-apierror) | Internal error |

## `GET /api/executions`

List all known executions (running and recently completed).

Tags: `executions`

### Responses

| Status | Content Type | Schema | Description |
| --- | --- | --- | --- |
| `200` | `application/json` | [`ExecutionList`](#schema-executionlist) | Execution list |
| `500` | `application/json` | [`ApiError`](#schema-apierror) | Internal error |

## `GET /api/executions/{id}`

Get the status and result of an execution.

Tags: `executions`

### Parameters

| Name | In | Required | Type | Description |
| --- | --- | --- | --- | --- |
| `id` | `path` | yes | string | Execution ID returned by POST /api/exec |

### Responses

| Status | Content Type | Schema | Description |
| --- | --- | --- | --- |
| `200` | `application/json` | [`ExecutionInfo`](#schema-executioninfo) | Execution found |
| `404` | `application/json` | [`ApiError`](#schema-apierror) | Execution not found |

## `POST /api/executions/{id}/cancel`

Cancel a running execution.

Tags: `executions`

### Parameters

| Name | In | Required | Type | Description |
| --- | --- | --- | --- | --- |
| `id` | `path` | yes | string | Execution ID to cancel |

### Responses

| Status | Content Type | Schema | Description |
| --- | --- | --- | --- |
| `200` | `application/json` | [`CancelResult`](#schema-cancelresult) | Cancel accepted |
| `400` | `application/json` | [`CancelResult`](#schema-cancelresult) | Cannot cancel (e.g. already finished) |

## `GET /api/executions/{id}/output`

Read paginated console output from an execution.

Supports both line-based (`line_offset` / `line_limit`) and byte-based (`byte_offset` / `byte_limit`) pagination.  Use `has_more` and `next_line_offset` / `next_byte_offset` to iterate.

Tags: `executions`

### Parameters

| Name | In | Required | Type | Description |
| --- | --- | --- | --- | --- |
| `id` | `path` | yes | string | Execution ID |
| `line_offset` | `query` | no | integer | null | Return output starting at this line number (0-indexed). |
| `line_limit` | `query` | no | integer | null | Maximum number of lines to return. |
| `byte_offset` | `query` | no | integer | null | Return output starting at this byte offset. |
| `byte_limit` | `query` | no | integer | null | Maximum number of bytes to return. |

### Responses

| Status | Content Type | Schema | Description |
| --- | --- | --- | --- |
| `200` | `application/json` | [`ExecutionOutput`](#schema-executionoutput) | Output page |
| `404` | `application/json` | [`ApiError`](#schema-apierror) | Execution not found |

## `GET /api/version`

Return the running server version.

Tags: `meta`

### Responses

| Status | Content Type | Schema | Description |
| --- | --- | --- | --- |
| `200` | - | - | Server version |

## Component Schemas

## Schema `ApiError`

Generic error body.

Type: `object`

| Property | Type | Required | Description |
| --- | --- | --- | --- |
| `error` | string | yes | - |

## Schema `CancelResult`

Result of a cancel request.

Type: `object`

| Property | Type | Required | Description |
| --- | --- | --- | --- |
| `error` | string | null | no | - |
| `ok` | boolean | yes | - |

## Schema `CliAsset`

A single entry in the CLI download index.

Type: `object`

| Property | Type | Required | Description |
| --- | --- | --- | --- |
| `available` | boolean | yes | Whether the binary is embedded in this server build. |
| `platform` | string | yes | Platform identifier (e.g. `linux-x86_64`). |
| `url` | string | yes | Download URL for this binary via the server itself. |

## Schema `CliIndex`

Index of available CLI binary downloads for the running server version.

Type: `object`

| Property | Type | Required | Description |
| --- | --- | --- | --- |
| `assets` | array<[`CliAsset`](#schema-cliasset)> | yes | Available platform binaries. |
| `version` | string | yes | Server (and CLI) version string, e.g. `"0.1.0"`. |

## Schema `ExecAccepted`

Accepted response containing the new execution's ID.

Type: `object`

| Property | Type | Required | Description |
| --- | --- | --- | --- |
| `execution_id` | string | yes | Unique identifier for the queued execution. |

## Schema `ExecRequest`

Request body for executing JavaScript code.

Type: `object`

| Property | Type | Required | Description |
| --- | --- | --- | --- |
| `code` | string | yes | JavaScript (or TypeScript) source code to execute. |
| `execution_timeout_secs` | integer | null | no | Per-execution timeout in seconds (overrides server default). |
| `heap` | string | null | no | Serialised heap snapshot key to restore before execution. |
| `heap_memory_max_mb` | integer | null | no | Per-execution V8 heap memory cap in megabytes. |
| `session` | string | null | no | Session identifier used for tagging / logging. |
| `tags` | object<string, string> | null | no | Arbitrary key/value tags attached to the resulting heap snapshot. |

## Schema `ExecutionInfo`

Detailed status of a single execution.

Type: `object`

| Property | Type | Required | Description |
| --- | --- | --- | --- |
| `completed_at` | string | null | no | ISO-8601 timestamp when execution finished (absent while running). |
| `error` | string | null | no | Error message (present when `status` is `failed`). |
| `execution_id` | string | yes | - |
| `heap` | string | null | no | Heap snapshot key produced after execution. |
| `result` | string | null | no | Final return value serialised to JSON (present when `status` is `completed`). |
| `started_at` | string | yes | ISO-8601 timestamp when execution started. |
| `status` | string | yes | Current status: `running`, `completed`, `failed`, `cancelled`, `timed_out`. |

## Schema `ExecutionList`

List of execution summaries.

Type: `object`

| Property | Type | Required | Description |
| --- | --- | --- | --- |
| `executions` | array<object> | yes | - |

## Schema `ExecutionOutput`

A page of console output from an execution.

Type: `object`

| Property | Type | Required | Description |
| --- | --- | --- | --- |
| `data` | string | yes | Text content for the requested window. |
| `end_byte` | integer | yes | Last byte offset in this page (exclusive). |
| `end_line` | integer | yes | Last line number in this page (exclusive). |
| `execution_id` | string | yes | - |
| `has_more` | boolean | yes | Whether more output is available beyond this page. |
| `next_byte_offset` | integer | yes | Byte offset to use for the next page (pass as `byte_offset`). |
| `next_line_offset` | integer | yes | Line offset to use for the next page (pass as `line_offset`). |
| `start_byte` | integer | yes | First byte offset in this page. |
| `start_line` | integer | yes | First line number in this page (0-indexed). |
| `status` | string | yes | Execution status at the time of this query. |
| `total_bytes` | integer | yes | Total bytes written so far. |
| `total_lines` | integer | yes | Total lines written so far. |
