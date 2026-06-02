# Tutorial: Setting Up OPA Policies

In this tutorial you will write Rego policies for Open Policy Agent (OPA) and configure mcp-v8 to enforce them. Policies control what JavaScript code is allowed to do -- which URLs it can fetch, which files it can access, which modules it can import, and more.

## Prerequisites

- mcp-v8 installed ([Installation](installation.md))
- mcp-v8-cli installed
- Basic understanding of JSON

## Step 1: Understand the policy system

mcp-v8 uses OPA (Open Policy Agent) policies written in Rego to gate access to sensitive operations. The policy system covers these areas:

| Section | Controls |
|---|---|
| `fetch` | HTTP requests via the Fetch API |
| `filesystem` | File read/write operations |
| `modules` | Module imports (npm, JSR, URL) |
| `subprocess` | Subprocess execution |
| `mcp_tools` | MCP pass-through tool calls |

Policies are evaluated locally within mcp-v8 -- no external OPA server is needed.

## Step 2: Write your first Rego policy

Create a file called `policies.json` that defines a policy allowing fetch requests only to specific domains:

```json
{
  "fetch": {
    "rego_source": "package mcp_v8.fetch\n\ndefault allow = false\n\nallow {\n  input.url_parsed.host == \"api.github.com\"\n}\n\nallow {\n  input.url_parsed.host == \"httpbin.org\"\n}"
  }
}
```

This policy:
- Denies all fetch requests by default (`default allow = false`)
- Allows requests to `api.github.com`
- Allows requests to `httpbin.org`

## Step 3: Understand the Rego policy structure

Let us break down the Rego source. Written more readably, it looks like this:

```rego
package mcp_v8.fetch

default allow = false

allow {
  input.url_parsed.host == "api.github.com"
}

allow {
  input.url_parsed.host == "httpbin.org"
}
```

Key concepts:
- `package mcp_v8.fetch` -- the package name must match the policy section
- `default allow = false` -- deny by default (secure default)
- `allow { ... }` -- each `allow` block is an OR condition; if any one matches, the request is allowed
- `input` -- contains the details of the operation being evaluated

For fetch policies, `input` includes:
- `input.method` -- HTTP method (GET, POST, etc.)
- `input.url` -- the full URL as a string
- `input.url_parsed` -- the parsed URL with fields like `host`, `path`, `scheme`
- `input.headers` -- request headers

## Step 4: Start the server with policies

```bash
mcp-v8 --http-port 3000 --policies-json policies.json
```

## Step 5: Test an allowed request

```bash
mcp-v8-cli --http-port 3000 exec --code '
const response = await fetch("https://httpbin.org/get");
const data = await response.json();
data.url;
'
```

Expected output:

```
https://httpbin.org/get
```

The request to httpbin.org is allowed by the policy.

## Step 6: Test a denied request

```bash
mcp-v8-cli --http-port 3000 exec --code '
const response = await fetch("https://example.com");
const text = await response.text();
text;
'
```

This will fail with a policy denial error, because `example.com` is not in the allowed hosts.

## Step 7: Write a filesystem policy

Add a filesystem section to your policies. Create `policies.json` with both fetch and filesystem rules:

```json
{
  "fetch": {
    "rego_source": "package mcp_v8.fetch\n\ndefault allow = false\n\nallow {\n  input.url_parsed.host == \"api.github.com\"\n}\n\nallow {\n  input.url_parsed.host == \"httpbin.org\"\n}"
  },
  "filesystem": {
    "rego_source": "package mcp_v8.filesystem\n\ndefault allow = false\n\nallow {\n  startswith(input.path, \"/tmp/\")\n  input.operation == \"read\"\n}\n\nallow {\n  startswith(input.path, \"/tmp/mcp-v8-data/\")\n}"
  }
}
```

This filesystem policy:
- Allows reading any file under `/tmp/`
- Allows reading and writing files under `/tmp/mcp-v8-data/`
- Denies all other filesystem access

## Step 8: Write a modules policy

Control which packages can be imported:

```json
{
  "modules": {
    "rego_source": "package mcp_v8.modules\n\ndefault allow = false\n\nallow {\n  startswith(input.specifier, \"npm:lodash\")\n}\n\nallow {\n  startswith(input.specifier, \"npm:date-fns\")\n}\n\nallow {\n  startswith(input.specifier, \"jsr:@std/\")\n}"
  }
}
```

This allows importing lodash, date-fns from npm, and any `@std` package from JSR, while blocking everything else.

## Step 9: Combine all policies

Create a comprehensive `policies.json`:

```json
{
  "fetch": {
    "rego_source": "package mcp_v8.fetch\n\ndefault allow = false\n\nallow {\n  input.url_parsed.host == \"api.github.com\"\n}\n\nallow {\n  input.url_parsed.host == \"httpbin.org\"\n}"
  },
  "filesystem": {
    "rego_source": "package mcp_v8.filesystem\n\ndefault allow = false\n\nallow {\n  startswith(input.path, \"/tmp/mcp-v8-data/\")\n}"
  },
  "modules": {
    "rego_source": "package mcp_v8.modules\n\ndefault allow = false\n\nallow {\n  startswith(input.specifier, \"npm:lodash\")\n}\n\nallow {\n  startswith(input.specifier, \"jsr:@std/\")\n}"
  },
  "subprocess": {
    "rego_source": "package mcp_v8.subprocess\n\ndefault allow = false"
  },
  "mcp_tools": {
    "rego_source": "package mcp_v8.mcp_tools\n\ndefault allow = false"
  }
}
```

This is a restrictive configuration that:
- Allows fetch only to two domains
- Allows filesystem access only under `/tmp/mcp-v8-data/`
- Allows only lodash and JSR standard library imports
- Denies all subprocess execution
- Denies all MCP tool pass-through calls

## Step 10: Restart and test the combined policies

```bash
mcp-v8 --http-port 3000 --policies-json policies.json
```

Test that allowed operations work:

```bash
mcp-v8-cli --http-port 3000 exec --code '
import _ from "npm:lodash@4";
_.sum([1, 2, 3, 4, 5]);
'
```

Expected output:

```
15
```

Test that denied operations are blocked:

```bash
mcp-v8-cli --http-port 3000 exec --code '
import axios from "npm:axios@1";
'
```

This will fail because axios is not in the allowed modules list.

## What you learned

- How OPA policies control access to fetch, filesystem, modules, subprocess, and MCP tools
- How to write Rego rules with the correct package names
- How to configure policies using `--policies-json`
- How the `input` object provides operation details for policy evaluation
- How to create a restrictive policy that denies by default and allows specific operations
- That policies are evaluated locally within mcp-v8

Next, learn about making HTTP requests in [Making HTTP Requests](network-access.md).
