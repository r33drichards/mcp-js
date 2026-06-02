# Tutorial: Automatic Credential Injection

In this tutorial you will configure mcp-v8 to automatically inject headers into outgoing fetch requests. You will set up static header injection and then configure OAuth2 client credentials flow with Keycloak for dynamic token injection.

## Prerequisites

- mcp-v8 installed ([Installation](installation.md))
- mcp-v8-cli installed
- Familiarity with fetch and policies ([Network Access](network-access.md))
- A Keycloak instance (for the OAuth2 section)

## Step 1: Understand header injection

Header injection lets you automatically add authentication headers to outgoing fetch requests without exposing credentials to the JavaScript code. This is critical for security -- AI-generated code never sees the actual tokens or API keys.

Two modes are supported:

1. **Static headers**: Fixed key-value pairs added to every matching request
2. **OAuth2 client credentials**: Dynamic tokens obtained from an OAuth2 provider (like Keycloak)

## Step 2: Configure static header injection

Create a configuration file called `headers-config.json`:

```json
{
  "fetch_header_injection": [
    {
      "url_pattern": "https://api.example.com/*",
      "headers": {
        "Authorization": "Bearer sk-my-static-api-key",
        "X-API-Version": "2024-01"
      }
    },
    {
      "url_pattern": "https://internal.company.com/*",
      "headers": {
        "X-Internal-Token": "internal-service-token-123"
      }
    }
  ]
}
```

This configuration:
- Adds an `Authorization` header and `X-API-Version` header to any request matching `https://api.example.com/*`
- Adds an `X-Internal-Token` header to any request matching `https://internal.company.com/*`

## Step 3: Create a fetch policy for the injected URLs

You still need a fetch policy to allow requests to these URLs. Create `policies.json`:

```json
{
  "fetch": {
    "rego_source": "package mcp_v8.fetch\n\ndefault allow = false\n\nallow {\n  input.url_parsed.host == \"api.example.com\"\n}\n\nallow {\n  input.url_parsed.host == \"internal.company.com\"\n}\n\nallow {\n  input.url_parsed.host == \"httpbin.org\"\n}"
  }
}
```

## Step 4: Start the server with header injection

```bash
mcp-v8 --http-port 3000 \
  --policies-json policies.json \
  --fetch-header-injection headers-config.json
```

## Step 5: Test static header injection

The JavaScript code does not need to know about the credentials. It just makes a normal fetch call:

```bash
mcp-v8-cli --http-port 3000 exec --code '
// No credentials in the code -- they are injected automatically
const response = await fetch("https://httpbin.org/get");
const data = await response.json();
data.headers;
'
```

If you use httpbin.org as a test target (and add it to your injection config), you can see the injected headers echoed back in the response.

## Step 6: Verify headers are injected transparently

```bash
mcp-v8-cli --http-port 3000 exec --code '
// The JavaScript code is completely unaware of the injected headers
const response = await fetch("https://api.example.com/data");
// The Authorization header is added before the request is sent
// The code never has access to the token
response.status;
'
```

The key security benefit: even if AI-generated code tried to exfiltrate credentials, it cannot access the injected headers because they are added at the transport layer, outside the V8 sandbox.

## Step 7: Configure OAuth2 client credentials with Keycloak

For dynamic tokens, configure OAuth2 client credentials flow. This is more complex but handles token refresh automatically.

Create `oauth-config.json`:

```json
{
  "fetch_header_injection": [
    {
      "url_pattern": "https://api.myservice.com/*",
      "oauth2": {
        "grant_type": "client_credentials",
        "token_url": "https://keycloak.example.com/realms/myrealm/protocol/openid-connect/token",
        "client_id": "mcp-v8-client",
        "client_secret": "your-client-secret-here",
        "scopes": ["api:read", "api:write"]
      }
    }
  ]
}
```

This configuration:
- Matches requests to `https://api.myservice.com/*`
- Obtains an access token from Keycloak using the client credentials grant
- Automatically injects the token as a `Bearer` token in the `Authorization` header
- Refreshes the token when it expires

## Step 8: Start the server with OAuth2 injection

```bash
mcp-v8 --http-port 3000 \
  --policies-json policies.json \
  --fetch-header-injection oauth-config.json
```

## Step 9: Use the OAuth2-protected API

The JavaScript code is identical to a normal fetch call:

```bash
mcp-v8-cli --http-port 3000 exec --code '
// mcp-v8 handles OAuth2 token acquisition and injection
const response = await fetch("https://api.myservice.com/resources");
const data = await response.json();
data;
'
```

Behind the scenes, mcp-v8:
1. Checks if it has a valid token for this URL pattern
2. If not, calls the Keycloak token endpoint with client credentials
3. Caches the token until it expires
4. Injects the `Authorization: Bearer <token>` header into the request

## Step 10: Combine static and OAuth2 injection

You can mix both approaches in a single configuration:

```json
{
  "fetch_header_injection": [
    {
      "url_pattern": "https://simple-api.example.com/*",
      "headers": {
        "X-API-Key": "static-key-123"
      }
    },
    {
      "url_pattern": "https://secure-api.example.com/*",
      "oauth2": {
        "grant_type": "client_credentials",
        "token_url": "https://keycloak.example.com/realms/myrealm/protocol/openid-connect/token",
        "client_id": "mcp-v8-client",
        "client_secret": "your-client-secret-here",
        "scopes": ["api:read"]
      }
    }
  ]
}
```

Different URL patterns can use different injection strategies.

## What you learned

- Header injection adds credentials to fetch requests at the transport layer, outside the V8 sandbox
- Static headers are simple key-value pairs injected for matching URL patterns
- OAuth2 client credentials flow dynamically obtains and refreshes tokens from providers like Keycloak
- JavaScript code never has access to the injected credentials
- You can combine static and OAuth2 injection for different URL patterns
- A fetch policy is still required to allow requests to the target URLs

Next, learn about filesystem access in [Reading and Writing Files](filesystem-access.md).
