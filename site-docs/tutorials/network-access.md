# Tutorial: Making HTTP Requests from JavaScript

In this tutorial you will enable the Fetch API in mcp-v8, write a policy to control which URLs are accessible, and use the standard Fetch API to make HTTP requests from JavaScript.

## Prerequisites

- mcp-v8 installed ([Installation](installation.md))
- mcp-v8-cli installed
- Familiarity with OPA policies ([Policy System](policy-system.md))

## Step 1: Create a fetch policy

The Fetch API is gated by OPA policy. Without a policy that allows it, all fetch requests are denied. Create a file called `fetch-policy.json`:

```json
{
  "fetch": {
    "rego_source": "package mcp_v8.fetch\n\ndefault allow = false\n\nallow {\n  input.method == \"GET\"\n  input.url_parsed.host == \"httpbin.org\"\n}\n\nallow {\n  input.url_parsed.host == \"jsonplaceholder.typicode.com\"\n}\n\nallow {\n  input.url_parsed.host == \"api.github.com\"\n  input.method == \"GET\"\n}"
  }
}
```

This policy allows:
- GET requests to httpbin.org
- Any method to jsonplaceholder.typicode.com (a free test API)
- GET requests to api.github.com

## Step 2: Start the server with the policy

```bash
mcp-v8 --http-port 3000 --policies-json fetch-policy.json
```

## Step 3: Make a simple GET request

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

The Fetch API in mcp-v8 follows the web standard -- the same API you use in browsers and Deno.

## Step 4: Fetch JSON data

Retrieve data from a REST API:

```bash
mcp-v8-cli --http-port 3000 exec --code '
const response = await fetch("https://jsonplaceholder.typicode.com/todos/1");
const todo = await response.json();
todo;
'
```

Expected output:

```json
{
  "userId": 1,
  "id": 1,
  "title": "delectus aut autem",
  "completed": false
}
```

## Step 5: Make a POST request

Send data with a POST request:

```bash
mcp-v8-cli --http-port 3000 exec --code '
const response = await fetch("https://jsonplaceholder.typicode.com/posts", {
  method: "POST",
  headers: {
    "Content-Type": "application/json",
  },
  body: JSON.stringify({
    title: "Hello from mcp-v8",
    body: "This post was created by JavaScript running in V8",
    userId: 1,
  }),
});

const result = await response.json();
result;
'
```

Expected output:

```json
{
  "title": "Hello from mcp-v8",
  "body": "This post was created by JavaScript running in V8",
  "userId": 1,
  "id": 101
}
```

## Step 6: Work with response headers and status

Inspect the full response:

```bash
mcp-v8-cli --http-port 3000 exec --code '
const response = await fetch("https://httpbin.org/get");

const info = {
  status: response.status,
  statusText: response.statusText,
  contentType: response.headers.get("content-type"),
  ok: response.ok,
};
info;
'
```

Expected output:

```json
{
  "status": 200,
  "statusText": "OK",
  "contentType": "application/json",
  "ok": true
}
```

## Step 7: Handle errors gracefully

Network errors and non-200 responses should be handled:

```bash
mcp-v8-cli --http-port 3000 exec --code '
const response = await fetch("https://httpbin.org/status/404");

if (!response.ok) {
  `Request failed with status ${response.status}`;
} else {
  await response.text();
}
'
```

Expected output:

```
Request failed with status 404
```

## Step 8: Fetch with custom headers

Add custom headers to your requests:

```bash
mcp-v8-cli --http-port 3000 exec --code '
const response = await fetch("https://httpbin.org/get", {
  headers: {
    "X-Custom-Header": "mcp-v8-tutorial",
    "Accept": "application/json",
  },
});

const data = await response.json();
data.headers;
'
```

The response from httpbin.org echoes back the headers you sent, so you can verify they were included.

## Step 9: Fetch the GitHub API

Query the GitHub API (read-only, as the policy allows only GET):

```bash
mcp-v8-cli --http-port 3000 exec --code '
const response = await fetch("https://api.github.com/repos/r33drichards/mcp-js");
const repo = await response.json();

({
  name: repo.name,
  description: repo.description,
  stars: repo.stargazers_count,
  language: repo.language,
});
'
```

## Step 10: Test policy denial

Try to fetch a URL not in the policy:

```bash
mcp-v8-cli --http-port 3000 exec --code '
const response = await fetch("https://example.com");
await response.text();
'
```

This will fail with a policy denial error. The policy system ensures that JavaScript code can only access the network endpoints you have explicitly authorized.

## What you learned

- The Fetch API in mcp-v8 follows the web standard
- All fetch requests require an OPA policy that allows them
- Policy input includes `method`, `url`, `url_parsed`, and `headers` for granular control
- You can make GET, POST, and other HTTP requests with headers and body
- Response objects provide status, headers, and body just like in a browser
- Requests to unauthorized URLs are denied by the policy system

Next, learn about automatic credential injection in [Automatic Credential Injection](fetch-header-injection.md).
