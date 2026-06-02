# HTTP API

> Generated from the repo root `openapi.json` with Widdershins. Do not edit
> this page by hand.

This page documents the HTTP surface exposed by `mcp-v8` from the committed
OpenAPI description.

## CLI


### List available CLI binary downloads for the running server version.


<a id="opIdcli_index_handler"></a>

`GET /api/cli`

Each `url` is a direct download from this server. `available: false` means
the binary was not embedded at build time (dev/local builds).

> Example responses

> 200 Response

```json
{
  "assets": [
    {
      "available": true,
      "platform": "string",
      "url": "string"
    }
  ],
  "version": "string"
}
```

<a id="list-available-cli-binary-downloads-for-the-running-server-version.-responses"></a>
#### Responses


|Status|Meaning|Description|Schema|
|---|---|---|---|
|200|[OK](https://tools.ietf.org/html/rfc7231#section-6.3.1)|CLI download index|[CliIndex](#schemacliindex)|

Authentication: none.


### Download the CLI binary for a specific platform directly from this server.


<a id="opIdcli_download_handler"></a>

`GET /api/cli/{platform}`

The binary is embedded at build time and always matches the running server
version. Returns 404 if the server was built without embedded binaries
(dev/local builds).

Supported platforms: `linux-x86_64`, `linux-aarch64`, `macos-aarch64`.

<a id="download-the-cli-binary-for-a-specific-platform-directly-from-this-server.-parameters"></a>
#### Parameters


|Name|In|Type|Required|Description|
|---|---|---|---|---|
|platform|path|string|true|Target platform (linux-x86_64 | linux-aarch64 | macos-aarch64)|

> Example responses

> 404 Response

```json
{
  "error": "string"
}
```

<a id="download-the-cli-binary-for-a-specific-platform-directly-from-this-server.-responses"></a>
#### Responses


|Status|Meaning|Description|Schema|
|---|---|---|---|
|200|[OK](https://tools.ietf.org/html/rfc7231#section-6.3.1)|CLI binary (application/octet-stream)|None|
|404|[Not Found](https://tools.ietf.org/html/rfc7231#section-6.5.4)|Unknown platform or binary not embedded|[ApiError](#schemaapierror)|

Authentication: none.


## Executions


### Submit JavaScript code for asynchronous execution.


<a id="opIdexec_handler"></a>

`POST /api/exec`

Returns immediately with an `execution_id`. Use `GET /api/executions/{id}`
to poll status and `GET /api/executions/{id}/output` to read console output.

> Body parameter

```json
{
  "code": "string",
  "execution_timeout_secs": 0,
  "heap": "string",
  "heap_memory_max_mb": 0,
  "session": "string",
  "tags": {
    "property1": "string",
    "property2": "string"
  }
}
```

<a id="submit-javascript-code-for-asynchronous-execution.-parameters"></a>
#### Parameters


|Name|In|Type|Required|Description|
|---|---|---|---|---|
|body|body|[ExecRequest](#schemaexecrequest)|true|none|

> Example responses

> 202 Response

```json
{
  "execution_id": "string"
}
```

<a id="submit-javascript-code-for-asynchronous-execution.-responses"></a>
#### Responses


|Status|Meaning|Description|Schema|
|---|---|---|---|
|202|[Accepted](https://tools.ietf.org/html/rfc7231#section-6.3.3)|Execution queued|[ExecAccepted](#schemaexecaccepted)|
|500|[Internal Server Error](https://tools.ietf.org/html/rfc7231#section-6.6.1)|Internal error|[ApiError](#schemaapierror)|

Authentication: none.


### List all known executions (running and recently completed).


<a id="opIdlist_executions_handler"></a>

`GET /api/executions`

> Example responses

> 200 Response

```json
{
  "executions": [
    null
  ]
}
```

<a id="list-all-known-executions-(running-and-recently-completed).-responses"></a>
#### Responses


|Status|Meaning|Description|Schema|
|---|---|---|---|
|200|[OK](https://tools.ietf.org/html/rfc7231#section-6.3.1)|Execution list|[ExecutionList](#schemaexecutionlist)|
|500|[Internal Server Error](https://tools.ietf.org/html/rfc7231#section-6.6.1)|Internal error|[ApiError](#schemaapierror)|

Authentication: none.


### Get the status and result of an execution.


<a id="opIdget_execution_handler"></a>

`GET /api/executions/{id}`

<a id="get-the-status-and-result-of-an-execution.-parameters"></a>
#### Parameters


|Name|In|Type|Required|Description|
|---|---|---|---|---|
|id|path|string|true|Execution ID returned by POST /api/exec|

> Example responses

> 200 Response

```json
{
  "completed_at": "string",
  "error": "string",
  "execution_id": "string",
  "heap": "string",
  "result": "string",
  "started_at": "string",
  "status": "string"
}
```

<a id="get-the-status-and-result-of-an-execution.-responses"></a>
#### Responses


|Status|Meaning|Description|Schema|
|---|---|---|---|
|200|[OK](https://tools.ietf.org/html/rfc7231#section-6.3.1)|Execution found|[ExecutionInfo](#schemaexecutioninfo)|
|404|[Not Found](https://tools.ietf.org/html/rfc7231#section-6.5.4)|Execution not found|[ApiError](#schemaapierror)|

Authentication: none.


### Cancel a running execution.


<a id="opIdcancel_execution_handler"></a>

`POST /api/executions/{id}/cancel`

<a id="cancel-a-running-execution.-parameters"></a>
#### Parameters


|Name|In|Type|Required|Description|
|---|---|---|---|---|
|id|path|string|true|Execution ID to cancel|

> Example responses

> 200 Response

```json
{
  "error": "string",
  "ok": true
}
```

<a id="cancel-a-running-execution.-responses"></a>
#### Responses


|Status|Meaning|Description|Schema|
|---|---|---|---|
|200|[OK](https://tools.ietf.org/html/rfc7231#section-6.3.1)|Cancel accepted|[CancelResult](#schemacancelresult)|
|400|[Bad Request](https://tools.ietf.org/html/rfc7231#section-6.5.1)|Cannot cancel (e.g. already finished)|[CancelResult](#schemacancelresult)|

Authentication: none.


### Read paginated console output from an execution.


<a id="opIdget_execution_output_handler"></a>

`GET /api/executions/{id}/output`

Supports both line-based (`line_offset` / `line_limit`) and byte-based
(`byte_offset` / `byte_limit`) pagination.  Use `has_more` and
`next_line_offset` / `next_byte_offset` to iterate.

<a id="read-paginated-console-output-from-an-execution.-parameters"></a>
#### Parameters


|Name|In|Type|Required|Description|
|---|---|---|---|---|
|id|path|string|true|Execution ID|
|line_offset|query|integer(int64)|false|Return output starting at this line number (0-indexed).|
|line_limit|query|integer(int64)|false|Maximum number of lines to return.|
|byte_offset|query|integer(int64)|false|Return output starting at this byte offset.|
|byte_limit|query|integer(int64)|false|Maximum number of bytes to return.|

> Example responses

> 200 Response

```json
{
  "data": "string",
  "end_byte": 0,
  "end_line": 0,
  "execution_id": "string",
  "has_more": true,
  "next_byte_offset": 0,
  "next_line_offset": 0,
  "start_byte": 0,
  "start_line": 0,
  "status": "string",
  "total_bytes": 0,
  "total_lines": 0
}
```

<a id="read-paginated-console-output-from-an-execution.-responses"></a>
#### Responses


|Status|Meaning|Description|Schema|
|---|---|---|---|
|200|[OK](https://tools.ietf.org/html/rfc7231#section-6.3.1)|Output page|[ExecutionOutput](#schemaexecutionoutput)|
|404|[Not Found](https://tools.ietf.org/html/rfc7231#section-6.5.4)|Execution not found|[ApiError](#schemaapierror)|

Authentication: none.


## Meta


### Return the running server version.


<a id="opIdversion_handler"></a>

`GET /api/version`

<a id="return-the-running-server-version.-responses"></a>
#### Responses


|Status|Meaning|Description|Schema|
|---|---|---|---|
|200|[OK](https://tools.ietf.org/html/rfc7231#section-6.3.1)|Server version|None|

Authentication: none.


# Schemas

<h2 id="tocS_ApiError">ApiError</h2>
<!-- backwards compatibility -->
<a id="schemaapierror"></a>
<a id="schema_ApiError"></a>
<a id="tocSapierror"></a>
<a id="tocsapierror"></a>

```json
{
  "error": "string"
}

```

Generic error body.

### Properties

|Name|Type|Required|Restrictions|Description|
|---|---|---|---|---|
|error|string|true|none|none|

<h2 id="tocS_CancelResult">CancelResult</h2>
<!-- backwards compatibility -->
<a id="schemacancelresult"></a>
<a id="schema_CancelResult"></a>
<a id="tocScancelresult"></a>
<a id="tocscancelresult"></a>

```json
{
  "error": "string",
  "ok": true
}

```

Result of a cancel request.

### Properties

|Name|Type|Required|Restrictions|Description|
|---|---|---|---|---|
|error|string¦null|false|none|none|
|ok|boolean|true|none|none|

<h2 id="tocS_CliAsset">CliAsset</h2>
<!-- backwards compatibility -->
<a id="schemacliasset"></a>
<a id="schema_CliAsset"></a>
<a id="tocScliasset"></a>
<a id="tocscliasset"></a>

```json
{
  "available": true,
  "platform": "string",
  "url": "string"
}

```

A single entry in the CLI download index.

### Properties

|Name|Type|Required|Restrictions|Description|
|---|---|---|---|---|
|available|boolean|true|none|Whether the binary is embedded in this server build.|
|platform|string|true|none|Platform identifier (e.g. `linux-x86_64`).|
|url|string|true|none|Download URL for this binary via the server itself.|

<h2 id="tocS_CliIndex">CliIndex</h2>
<!-- backwards compatibility -->
<a id="schemacliindex"></a>
<a id="schema_CliIndex"></a>
<a id="tocScliindex"></a>
<a id="tocscliindex"></a>

```json
{
  "assets": [
    {
      "available": true,
      "platform": "string",
      "url": "string"
    }
  ],
  "version": "string"
}

```

Index of available CLI binary downloads for the running server version.

### Properties

|Name|Type|Required|Restrictions|Description|
|---|---|---|---|---|
|assets|[[CliAsset](#schemacliasset)]|true|none|Available platform binaries.|
|version|string|true|none|Server (and CLI) version string, e.g. `"0.1.0"`.|

<h2 id="tocS_ExecAccepted">ExecAccepted</h2>
<!-- backwards compatibility -->
<a id="schemaexecaccepted"></a>
<a id="schema_ExecAccepted"></a>
<a id="tocSexecaccepted"></a>
<a id="tocsexecaccepted"></a>

```json
{
  "execution_id": "string"
}

```

Accepted response containing the new execution's ID.

### Properties

|Name|Type|Required|Restrictions|Description|
|---|---|---|---|---|
|execution_id|string|true|none|Unique identifier for the queued execution.|

<h2 id="tocS_ExecRequest">ExecRequest</h2>
<!-- backwards compatibility -->
<a id="schemaexecrequest"></a>
<a id="schema_ExecRequest"></a>
<a id="tocSexecrequest"></a>
<a id="tocsexecrequest"></a>

```json
{
  "code": "string",
  "execution_timeout_secs": 0,
  "heap": "string",
  "heap_memory_max_mb": 0,
  "session": "string",
  "tags": {
    "property1": "string",
    "property2": "string"
  }
}

```

Request body for executing JavaScript code.

### Properties

|Name|Type|Required|Restrictions|Description|
|---|---|---|---|---|
|code|string|true|none|JavaScript (or TypeScript) source code to execute.|
|execution_timeout_secs|integer(int64)¦null|false|none|Per-execution timeout in seconds (overrides server default).|
|heap|string¦null|false|none|Serialised heap snapshot key to restore before execution.|
|heap_memory_max_mb|integer¦null|false|none|Per-execution V8 heap memory cap in megabytes.|
|session|string¦null|false|none|Session identifier used for tagging / logging.|
|tags|object¦null|false|none|Arbitrary key/value tags attached to the resulting heap snapshot.|
|» **additionalProperties**|string|false|none|none|

<h2 id="tocS_ExecutionInfo">ExecutionInfo</h2>
<!-- backwards compatibility -->
<a id="schemaexecutioninfo"></a>
<a id="schema_ExecutionInfo"></a>
<a id="tocSexecutioninfo"></a>
<a id="tocsexecutioninfo"></a>

```json
{
  "completed_at": "string",
  "error": "string",
  "execution_id": "string",
  "heap": "string",
  "result": "string",
  "started_at": "string",
  "status": "string"
}

```

Detailed status of a single execution.

### Properties

|Name|Type|Required|Restrictions|Description|
|---|---|---|---|---|
|completed_at|string¦null|false|none|ISO-8601 timestamp when execution finished (absent while running).|
|error|string¦null|false|none|Error message (present when `status` is `failed`).|
|execution_id|string|true|none|none|
|heap|string¦null|false|none|Heap snapshot key produced after execution.|
|result|string¦null|false|none|Final return value serialised to JSON (present when `status` is `completed`).|
|started_at|string|true|none|ISO-8601 timestamp when execution started.|
|status|string|true|none|Current status: `running`, `completed`, `failed`, `cancelled`, `timed_out`.|

<h2 id="tocS_ExecutionList">ExecutionList</h2>
<!-- backwards compatibility -->
<a id="schemaexecutionlist"></a>
<a id="schema_ExecutionList"></a>
<a id="tocSexecutionlist"></a>
<a id="tocsexecutionlist"></a>

```json
{
  "executions": [
    null
  ]
}

```

List of execution summaries.

### Properties

|Name|Type|Required|Restrictions|Description|
|---|---|---|---|---|
|executions|[any]|true|none|none|

<h2 id="tocS_ExecutionOutput">ExecutionOutput</h2>
<!-- backwards compatibility -->
<a id="schemaexecutionoutput"></a>
<a id="schema_ExecutionOutput"></a>
<a id="tocSexecutionoutput"></a>
<a id="tocsexecutionoutput"></a>

```json
{
  "data": "string",
  "end_byte": 0,
  "end_line": 0,
  "execution_id": "string",
  "has_more": true,
  "next_byte_offset": 0,
  "next_line_offset": 0,
  "start_byte": 0,
  "start_line": 0,
  "status": "string",
  "total_bytes": 0,
  "total_lines": 0
}

```

A page of console output from an execution.

### Properties

|Name|Type|Required|Restrictions|Description|
|---|---|---|---|---|
|data|string|true|none|Text content for the requested window.|
|end_byte|integer(int64)|true|none|Last byte offset in this page (exclusive).|
|end_line|integer(int64)|true|none|Last line number in this page (exclusive).|
|execution_id|string|true|none|none|
|has_more|boolean|true|none|Whether more output is available beyond this page.|
|next_byte_offset|integer(int64)|true|none|Byte offset to use for the next page (pass as `byte_offset`).|
|next_line_offset|integer(int64)|true|none|Line offset to use for the next page (pass as `line_offset`).|
|start_byte|integer(int64)|true|none|First byte offset in this page.|
|start_line|integer(int64)|true|none|First line number in this page (0-indexed).|
|status|string|true|none|Execution status at the time of this query.|
|total_bytes|integer(int64)|true|none|Total bytes written so far.|
|total_lines|integer(int64)|true|none|Total lines written so far.|

<h2 id="tocS_ExecutionSummary">ExecutionSummary</h2>
<!-- backwards compatibility -->
<a id="schemaexecutionsummary"></a>
<a id="schema_ExecutionSummary"></a>
<a id="tocSexecutionsummary"></a>
<a id="tocsexecutionsummary"></a>

```json
{
  "completed_at": "string",
  "execution_id": "string",
  "started_at": "string",
  "status": "string"
}

```

A brief summary of a single execution (used in list responses).

### Properties

|Name|Type|Required|Restrictions|Description|
|---|---|---|---|---|
|completed_at|string¦null|false|none|none|
|execution_id|string|true|none|none|
|started_at|string|true|none|none|
|status|string|true|none|none|

<h2 id="tocS_OutputQuery">OutputQuery</h2>
<!-- backwards compatibility -->
<a id="schemaoutputquery"></a>
<a id="schema_OutputQuery"></a>
<a id="tocSoutputquery"></a>
<a id="tocsoutputquery"></a>

```json
{
  "byte_limit": 0,
  "byte_offset": 0,
  "line_limit": 0,
  "line_offset": 0
}

```

Optional pagination query parameters for console output.

### Properties

|Name|Type|Required|Restrictions|Description|
|---|---|---|---|---|
|byte_limit|integer(int64)¦null|false|none|Maximum number of bytes to return.|
|byte_offset|integer(int64)¦null|false|none|Return output starting at this byte offset.|
|line_limit|integer(int64)¦null|false|none|Maximum number of lines to return.|
|line_offset|integer(int64)¦null|false|none|Return output starting at this line number (0-indexed).|
