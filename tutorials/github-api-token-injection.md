# GitHub API Token Injection

This tutorial shows how to configure mcp-js to inject a GitHub personal access token into outgoing `fetch()` requests, so that JavaScript code executed in the sandbox can call the GitHub API without ever seeing the token directly.

## How It Works

mcp-js supports **header injection rules** via the `--fetch-header` CLI flag. When a fetch request matches a rule's host (and optionally HTTP method), the server automatically injects the configured headers into the outgoing request. The injected headers are invisible to user-supplied JavaScript — they cannot be read or leaked by the sandbox code.

## Prerequisites

- Docker and Docker Compose
- A GitHub personal access token (classic or fine-grained)

## Step 1: Configure `docker-compose.yml`

Add a `--fetch-header` argument to the mcp-js service command. The format is:

```
host=<hostname>,header=<header-name>,value=<header-value>
```

Here is a full example using an environment variable for the token:

```yaml
services:
  opa:
    image: openpolicyagent/opa:latest
    command: ["run", "--server", "--addr", "0.0.0.0:8181", "/policies"]
    ports:
      - "8181:8181"
    volumes:
      - ./policies:/policies:ro

  mcp-js:
    build: .
    command:
      - --http-port=3000
      - --directory-path=/data/heaps
      - --session-db-path=/data/sessions
      - --opa-url=http://opa:8181
      - --fetch-header=host=api.github.com,header=Authorization,value=Bearer ${GITHUB_TOKEN}
    tmpfs:
      - /data:uid=1000,gid=1000
    ports:
      - "3000:3000"
    depends_on:
      - opa
```

The `${GITHUB_TOKEN}` variable is read from your shell environment or from a `.env` file in the same directory as the compose file.

### Using a `.env` file

Create a `.env` file (make sure it is in `.gitignore`):

```bash
echo 'GITHUB_TOKEN=ghp_yourTokenHere' > .env
```

## Step 2: Start the Services

```bash
docker compose up --build -d
```

Verify the header injection rule was loaded by checking the logs:

```bash
docker compose logs mcp-js | grep "header injection"
```

You should see:

```
Loaded 1 fetch header injection rule(s)
```

## Step 3: Test the Token

Use the `/api/exec` endpoint to execute JavaScript that calls the GitHub API. Note that you must include a `User-Agent` header in the fetch request — GitHub requires it.

### List repositories for a user

```bash
curl -s -X POST http://localhost:3000/api/exec \
  -H "Content-Type: application/json" \
  -d '{
    "code": "(async () => { const r = await fetch(\"https://api.github.com/users/r33drichards/repos?per_page=5&sort=updated\", { headers: { \"Accept\": \"application/vnd.github+json\", \"User-Agent\": \"mcp-js-test\" } }); const data = await r.json(); return JSON.stringify(data.map(repo => ({ name: repo.full_name, private: repo.private, url: repo.html_url })), null, 2); })()"
  }'
```

Example response:

```json
{
  "output": "[\n  {\n    \"name\": \"r33drichards/mcp-registry\",\n    \"private\": false,\n    \"url\": \"https://github.com/r33drichards/mcp-registry\"\n  },\n  {\n    \"name\": \"r33drichards/mcp-js\",\n    \"private\": false,\n    \"url\": \"https://github.com/r33drichards/mcp-js\"\n  },\n  {\n    \"name\": \"r33drichards/headlessmc-bot\",\n    \"private\": false,\n    \"url\": \"https://github.com/r33drichards/headlessmc-bot\"\n  },\n  {\n    \"name\": \"r33drichards/mcp-tlaplus\",\n    \"private\": false,\n    \"url\": \"https://github.com/r33drichards/mcp-tlaplus\"\n  },\n  {\n    \"name\": \"r33drichards/darwin\",\n    \"private\": false,\n    \"url\": \"https://github.com/r33drichards/darwin\"\n  }\n]"
}
```

### Check the authenticated user

```bash
curl -s -X POST http://localhost:3000/api/exec \
  -H "Content-Type: application/json" \
  -d '{
    "code": "(async () => { const r = await fetch(\"https://api.github.com/user\", { headers: { \"User-Agent\": \"mcp-js-test\" } }); const data = await r.json(); return JSON.stringify({ login: data.login, name: data.name }, null, 2); })()"
  }'
```

## Alternative: JSON Config File

For multiple header rules or more complex configurations, use `--fetch-header-config` with a JSON file:

```json
[
  {
    "host": "api.github.com",
    "methods": ["GET", "POST"],
    "headers": {
      "Authorization": "Bearer ghp_yourTokenHere",
      "X-GitHub-Api-Version": "2022-11-28"
    }
  }
]
```

Mount the file and reference it in your compose command:

```yaml
    command:
      - --http-port=3000
      - --opa-url=http://opa:8181
      - --fetch-header-config=/config/headers.json
    volumes:
      - ./headers.json:/config/headers.json:ro
```

## Important Notes

- **User-provided headers take precedence.** If the JavaScript code explicitly sets an `Authorization` header in a fetch call, the injected header will not override it.
- **Wildcard hosts are supported.** You can use `*.github.com` to match all GitHub subdomains.
- **OPA policy must allow the domain.** The default policy already allows `api.github.com`. If you have a custom policy, make sure it permits requests to GitHub.
- **Never commit tokens.** Use environment variables or a `.env` file (added to `.gitignore`) to keep secrets out of version control.
