# Run a stateful JavaScript interpreter on Modal

Deploy the published `mcp-js` Docker image once, then submit JavaScript
whenever you need it. Use the same terminal for both steps. When the
interpreter is idle, Modal can stop its container while keeping the endpoint
available. The next request starts the container again. The walkthrough
uses the same stateful pattern as Modal’s interpreter example, with V8
running the snippets. [Original Modal example](https://modal.com/docs/examples/simple_code_interpreter).

## 1. Set up the Modal CLI

Open a Bash terminal. You need a Modal account and Python 3.10 or later.
Create a virtual environment, install the CLI, and sign in.

```sh
python -m venv .venv
source .venv/bin/activate
python -m pip install 'modal>=1.6.4'
modal setup
```

`modal setup` signs this terminal’s CLI into your Modal workspace so it can
create the app and manage volumes. Keep the virtual environment active for
the remaining steps. [Modal setup guide](https://modal.com/docs/guide).

## 2. Deploy the interpreter

Run the commands in this section in the same Bash terminal. First make
a local directory for the policy files. You'll upload them to Modal
before deploying the image.

```bash
policy_dir=$(mktemp -d)
printf "Policy files: %s\n" "$policy_dir"
```

### 2.1. Give JavaScript access to the workspace

Write a filesystem policy that allows reading, writing, listing, and
creating directories under `/workspace`. The rule checks the requested
operation and path on each call. It rejects `..` path components, and the
separate rule for `/workspace` itself lets JavaScript list the volume root.
[Filesystem policy inputs and path handling](../concepts/filesystem.md).

```bash
cat >"$policy_dir/filesystem.rego" <<'REGO'
package mcp.filesystem

default allow := false

allow if {
    input.operation in {"readFile", "writeFile", "mkdir", "readdir"}
    startswith(input.path, "/workspace/")
    not ".." in split(input.path, "/")
}

allow if {
    input.operation == "readdir"
    input.path == "/workspace"
}
REGO
```

### 2.2. Allow outbound fetch to one host

The fetch policy permits GET requests to `https://example.com` on the
default HTTPS port. The server evaluates the method, scheme, host, and
port before sending a request. To use your own API, change these conditions
to match the requests it needs. [Fetch policy examples](../how-to/fetch.md).

```bash
cat >"$policy_dir/fetch.rego" <<'REGO'
package mcp.fetch

default allow := false

allow if {
    input.method == "GET"
    input.url_parsed.scheme == "https"
    input.url_parsed.host == "example.com"
    input.url_parsed.port == null
}
REGO
```

### 2.3. Allow one external module

The module policy checks the resolved import URL. This rule allows a
specific version of `escape-html` from jsDelivr. The deployment command
also enables the external module loader. [External module imports](../how-to/module-imports.md).

```bash
cat >"$policy_dir/modules.rego" <<'REGO'
package mcp.modules

default allow := false

allow if {
    input.resolved_url == "https://cdn.jsdelivr.net/npm/escape-html@1.0.3/+esm"
}
REGO
```

### 2.4. Upload the policies and tell mcp-v8 where to find them

Create a Modal Volume for the policy files, then copy the three local
files to its root. The create call reuses the volume if it already
exists. The upload replaces files with the same names so you can repeat
this step after editing a rule. [Modal Volumes](https://modal.com/docs/guide/volumes).

```bash
python - <<'PYTHON'
import modal

modal.Volume.objects.create("mcp-js-interpreter-policies", allow_existing=True)
PYTHON

for name in filesystem fetch modules; do
  modal volume put --force mcp-js-interpreter-policies "$policy_dir/$name.rego" "/$name.rego"
done
```

Define the policy configuration in the shell. The paths below are inside
the server container, where the next step mounts the policy volume at
`/policies`. At startup, mcp-v8 reads the Rego files at these paths and
evaluates them in-process. [mcp-v8 policy evaluation](../concepts/policies.md#opa-vs-embedded-regorus).

```bash
policies='{
  "filesystem": {"policies": [{"url": "file:///policies/filesystem.rego"}]},
  "fetch": {"policies": [{"url": "file:///policies/fetch.rego"}]},
  "modules": {"policies": [{"url": "file:///policies/modules.rego"}]}
}'
```

### 2.5. Deploy the image and mount the volumes

Run `modal server deploy` with the published image. Modal uses the image’s
entrypoint to start mcp-v8 on port 8080. The deployment allows zero warm
containers when idle and at most one active replica writing to the volumes.
[Modal Servers](https://modal.com/docs/guide/servers).

| Path in the container | What it holds |
| --- | --- |
| `/data` | V8 heap snapshots and the records that tie them to session names. |
| `/workspace` | Files JavaScript can access through the filesystem policy. |
| `/policies` | The three Rego files you uploaded. |

```bash
modal server deploy \
  --app-name mcp-js-code-interpreter \
  --image docker.io/wholelottahoopla/mcp-js:0.22.0 \
  --port 8080 \
  --min-containers 0 \
  --max-containers 1 \
  --scaledown-window 60 \
  --volume mcp-js-interpreter-state:/data \
  --volume mcp-js-interpreter-workspace:/workspace \
  --volume mcp-js-interpreter-policies:/policies \
  --env-var "MCP_V8_POLICIES_JSON=$policies" \
  --env-var MCP_V8_ALLOW_EXTERNAL_MODULES=true \
  --env-var PORT=8080 \
  --env-var MCP_V8_HEAP_STORE=dir \
  --env-var MCP_V8_HEAP_DIR=/data/heaps \
  --env-var MCP_V8_SESSION_DB_PATH=/data/sessions
```

Modal prints an HTTPS URL and the deploy command exits. Save the URL for
Step 3. You can inspect the deployed server’s mounts and autoscaling
settings with this command.

```sh
modal server info mcp-js-code-interpreter/Server
```

## 3. Authenticate and run JavaScript

In the same terminal, create a Modal Proxy Token for the HTTP endpoint. You can reuse one you already have for the environment.

```sh
modal workspace proxy-tokens create --name mcp-js-interpreter
```

The CLI displays a proxy token ID beginning with `wk-` and a secret
beginning with `ws-`. Set the URL from deployment, then paste the ID and
secret joined by a dot when prompted. Bash reads the token without echoing
it. The client passes it in the HTTP `Authorization` header. Keep the token
private. [Modal proxy authentication](https://modal.com/docs/guide/webhook-proxy-auth).

```sh
export MCP_JS_URL='https://YOUR-DEPLOYED-URL.modal.run'
read -rs -p 'Modal proxy token (wk-ID.ws-SECRET): ' MCP_JS_TOKEN
echo
export MCP_JS_TOKEN
```

Save this client as `run_code.py`. It uses the Python standard library.

```python
"""Submit JavaScript to a deployed Modal interpreter."""

import argparse
import json
import os
import time
from functools import partial
from urllib.error import URLError
from urllib.request import Request, urlopen


def request(base_url, token, path, body=None):
    headers = {"Authorization": f"Bearer {token}"}
    data = None
    if body is not None:
        headers["Content-Type"] = "application/json"
        data = json.dumps(body).encode()
    with urlopen(Request(base_url.rstrip("/") + path, data, headers), timeout=60) as response:
        return json.load(response)


def run_js(call, code, *, session="tutorial"):
    execution = call(
        "/api/exec",
        {"code": code, "session": session, "execution_timeout_secs": 5},
    )
    path = f"/api/executions/{execution['execution_id']}"
    deadline = time.monotonic() + 90
    while time.monotonic() < deadline:
        result = call(path)
        status = result["status"].lower()
        if status in {"completed", "failed", "timed_out", "cancelled"}:
            output = call(f"{path}/output")["data"]
            if output:
                print(output.rstrip())
            if status != "completed" or result.get("error"):
                raise RuntimeError(result.get("error") or status)
            return
        time.sleep(0.25)
    raise TimeoutError("The execution did not finish before the client deadline")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("code", help="JavaScript source")
    parser.add_argument("--session", default="tutorial", help="Session name")
    args = parser.parse_args()

    url = os.environ.get("MCP_JS_URL")
    token = os.environ.get("MCP_JS_TOKEN")
    if not url or not token:
        parser.error("Set MCP_JS_URL to the deployed URL and MCP_JS_TOKEN to wk-ID.ws-SECRET.")

    call = partial(request, url, token)
    try:
        run_js(call, args.code, session=args.session)
    except (RuntimeError, URLError, TimeoutError) as error:
        parser.exit(1, f"{error}\n")


if __name__ == "__main__":
    main()
```

Run the commands in order. Each command submits a snippet with
`POST /api/exec`, then uses the returned ID to poll for completion and read
console output. The first request might take longer while Modal starts
the container. [Asynchronous execution](../concepts/async-execution.md) ·
[HTTP API reference](../reference/http-api.md).

```sh
python run_code.py 'globalThis.x = 10'
python run_code.py 'globalThis.y = 5'
python run_code.py 'console.log(x + y)'
```

The first command sets `x`. The second restores the same heap and sets
`y`. The third restores both values and prints `15`. All three use
the default session name `tutorial`. Try the same read under a new name.
It exits with a reference error because that session starts with a fresh
heap. [Sessions and heap snapshots](../concepts/sessions-and-heaps.md).

```sh
python run_code.py --session fresh 'console.log(x + y)'
```

## 4. Read and write files on the workspace volume

The deployment command mounts `mcp-js-interpreter-workspace` at `/workspace`.
The separate `/data` volume holds interpreter state, while `/policies`
holds the Rego files. `MCP_V8_POLICIES_JSON` maps each capability to the
file that controls it.

Mounting a volume makes it visible to the server process. The
`mcp.filesystem` policy lets JavaScript list directories, create
directories, and read or write files anywhere under `/workspace`,
including subdirectories. Each `fs` call is checked before the server
touches the filesystem.

```sh
python run_code.py 'await fs.mkdir("/workspace/reports", {recursive: true})'
python run_code.py 'await fs.writeFile("/workspace/reports/summary.txt", "hello from Modal")'
python run_code.py 'console.log(await fs.readdir("/workspace"))'
python run_code.py 'console.log(await fs.readFile("/workspace/reports/summary.txt", "utf8"))'
```

The first command creates a subdirectory. The second writes a file there.
The third lists the volume root and includes `reports`. The fourth reads the
file as UTF-8 and prints `hello from Modal`. The rule uses the `/workspace/`
prefix so a sibling such as `/workspace-other` stays outside the grant. It also rejects any
`..` path component. Rego checks the supplied path before filesystem access,
and the OS can still follow symlinks. Keep the dedicated workspace volume
free of symlinks that point outside it. See
[filesystem policy inputs and path handling](../concepts/filesystem.md).

## 5. Fetch data and import a module

The `mcp.fetch` rule permits HTTPS GET requests to `example.com` on its
default port. Changing the method to POST or the host to another domain
causes a policy denial.

```sh
python run_code.py 'console.log((await fetch("https://example.com/")).status)'
```

The command prints the HTTP status from the upstream site. The policy
checks the request fields before the server sends it. To allow your own
API, edit the host in `fetch.rego` and add path or method conditions as needed. [Fetch policy examples](../how-to/fetch.md).

To load an external module, the deployment enables
`MCP_V8_ALLOW_EXTERNAL_MODULES` and the `mcp.modules` policy permits one
exact URL. The module loader checks this separately from `fetch()`.

```sh
python run_code.py 'import escape from "https://cdn.jsdelivr.net/npm/escape-html@1.0.3/+esm"; console.log(escape("<tag>"))'
```

The import example prints `&lt;tag&gt;`. To grant another capability, add
its Rego file to the upload list in Step 2.4 and register it under
the matching key in `policies`. [External module imports](../how-to/module-imports.md) ·
[Recipes for other capabilities](../how-to/policies.md) ·
[Policy input fields](../concepts/policies.md#per-category-input-documents).

After editing a policy, repeat Steps 2.4 and 2.5 and confirm the redeploy.
A newly started server loads the uploaded rules. Keep the
policy files on their separate volume so the workspace rule cannot modify
them.

## 6. Check scale-to-zero and stop the app

After roughly 60 seconds without requests, Modal can stop the container.
Wait until the deployment shows zero containers in the Modal dashboard,
then run the read again with the default session.

```sh
python run_code.py 'console.log(x + y)'
```

This request starts a container and should print `15` from the saved heap.
Modal commits volume changes in the background and on normal shutdown.
The new container can restore committed state. A sudden interruption can
lose writes since the last commit. [Modal autoscaling](https://modal.com/docs/guide/servers) ·
[Modal Volume commits](https://modal.com/docs/guide/volumes).

When you’re done with the endpoint, stop the app.

```sh
modal app stop mcp-js-code-interpreter
```

The volumes remain available for a later deployment.
