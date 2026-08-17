# Deploying mcp-v8 on Railway

This repo ships a [`railway.json`](./railway.json) config-as-code file, so a
Railway service pointed at this repository builds from the [`Dockerfile`](./Dockerfile)
and health-checks `GET /api/version` with no dashboard configuration. This page
covers the one-off deploy, plus the recipe for packaging it as a reusable
[Railway template](https://docs.railway.com/templates/create).

<!-- Once the template is created and published, put the real template code in
     this button link (https://railway.com/new/template/<CODE>) and mirror it in
     the README. -->
[![Deploy on Railway](https://railway.com/button.svg)](https://railway.com/new/template)

## What you get

A single service running the mcp-v8 server over Streamable HTTP:

- **MCP endpoint** — `POST https://<your-domain>/mcp`
- **REST sidecar** — `POST https://<your-domain>/api/exec`, plus the
  `/api/executions`, `/api/fs/...` and `/docs` routes
- **Durable state** — a Railway volume mounted at `/data` holds heap
  snapshots, the persistent `/work` filesystem, and the session/execution
  database, so agent state survives redeploys
- **Sandboxed by default** — scripts already run in a V8 isolate with no
  network, filesystem, or subprocess ops; the variables below additionally
  enable every in-process [hardening flag](https://r33drichards.github.io/mcp-js/reference/cli-flags/)
  and confine the whole server process with the kernel-enforced
  [OS sandbox](https://r33drichards.github.io/mcp-js/how-to/os-sandbox/)
  (Landlock), which blocks all outbound network egress

Railway injects `PORT`; the Docker image folds it into `--http-port`
automatically, so no start command is needed. The image also sets
`MCP_V8_ALLOWED_HOSTS=*`, which the variables below narrow back down to the
service's own Railway domains.

## One-off deploy (no template)

1. **New Project → Deploy from GitHub repo** and pick this repository
   (or your fork). `railway.json` supplies the builder, healthcheck, and
   restart policy.
2. **Attach a volume**: right-click the service → **Attach Volume**, mount
   path `/data`.
3. **Set variables** (Variables tab → Raw Editor):

   ```env
   RAILWAY_RUN_UID=0
   MCP_V8_HEAP_STORE=dir
   MCP_V8_HEAP_DIR=/data/heaps
   MCP_V8_FS_STORE=dir
   MCP_V8_FS_DIR=/data/fs
   MCP_V8_SESSION_DB_PATH=/data/sessions
   MCP_V8_ALLOWED_HOSTS=${{RAILWAY_PUBLIC_DOMAIN}},${{RAILWAY_PRIVATE_DOMAIN}}
   MCP_V8_SANDBOX_MANIFEST={"version":"0.1.0","network":{"mode":"blocked"}}
   MCP_V8_HARDEN_FREEZE_OPS=true
   MCP_V8_HARDEN_NEUTRALIZE_PROXY_DETAILS=true
   MCP_V8_HARDEN_NEUTRALIZE_INTROSPECTION=true
   MCP_V8_HARDEN_REMOVE_BOOTSTRAP=true
   MCP_V8_HARDEN_REMOVE_SHARED_MEMORY=true
   ```

4. **Enable public networking**: Settings tab → Networking → Generate Domain
   (target port 8080, or leave it to detect `PORT`).
5. Deploy. When the healthcheck passes, connect a client:

   ```bash
   claude mcp add --transport http js https://<your-domain>/mcp
   ```

   or exercise the REST sidecar directly:

   ```bash
   curl -X POST https://<your-domain>/api/exec \
     -H 'Content-Type: application/javascript' \
     --data-binary 'console.log([1,2,3].map(x => x * 2))'
   ```

## Creating the template

Railway templates are composed in the dashboard, not from a file in the repo —
see [Create a Template](https://docs.railway.com/templates/create). From your
workspace's **Templates** page, click **New Template**, then:

1. **Add a service** with source repo `https://github.com/r33drichards/mcp-js`
   (append `/tree/<branch>` to pin a branch; default is `main`).
2. **Variables tab** — add the variables from the table below, with
   descriptions so deployers know what each one does.
3. **Settings tab** — enable **Public Networking** (HTTP). The healthcheck
   path and restart policy come from `railway.json`, so they don't need to be
   set here; setting the healthcheck to `/api/version` anyway is harmless.
4. **Right-click the service → Attach Volume**, mount path `/data`.
5. **Create Template**, then optionally
   [publish it](https://docs.railway.com/templates/publish-and-share) to the
   marketplace and drop the resulting template code into the deploy button at
   the top of this file and in the README.

### Template variables

| Variable | Value | Why |
| --- | --- | --- |
| `RAILWAY_RUN_UID` | `0` | Railway mounts volumes owned by `root`; the image runs as a non-root user, so without this the server cannot write to `/data`. |
| `MCP_V8_HEAP_STORE` | `dir` | Persist V8 heap snapshots (stateful mode) instead of the default no-persistence mode. |
| `MCP_V8_HEAP_DIR` | `/data/heaps` | Keep heap snapshots on the volume. |
| `MCP_V8_FS_STORE` | `dir` | Persist the content-addressed `/work` filesystem. |
| `MCP_V8_FS_DIR` | `/data/fs` | Keep `/work` snapshots on the volume. |
| `MCP_V8_SESSION_DB_PATH` | `/data/sessions` | Session log + async-execution registry on the volume (default is `/tmp`, which is wiped on redeploy). |
| `MCP_V8_ALLOWED_HOSTS` | `${{RAILWAY_PUBLIC_DOMAIN}},${{RAILWAY_PRIVATE_DOMAIN}}` | Narrows the image's `*` Host allowlist to the domains Railway actually serves, restoring DNS-rebinding protection on `/mcp`. |
| `MCP_V8_SANDBOX_MANIFEST` | `{"version":"0.1.0","network":{"mode":"blocked"}}` | Confines the whole server process with the kernel-enforced [OS sandbox](https://r33drichards.github.io/mcp-js/how-to/os-sandbox/) (Landlock): filesystem access is limited to the server's own storage/config paths and all outbound network egress is blocked. The listener port and the `/data` directories are granted automatically. |
| `MCP_V8_HARDEN_FREEZE_OPS` | `true` | Freeze `Deno.core.ops` so user code cannot replace or intercept ops (e.g. a trojan op persisting across heap snapshots). |
| `MCP_V8_HARDEN_NEUTRALIZE_PROXY_DETAILS` | `true` | Neutralize `op_get_proxy_details`, which otherwise bypasses `Proxy` handlers. |
| `MCP_V8_HARDEN_NEUTRALIZE_INTROSPECTION` | `true` | Neutralize `op_memory_usage` + `op_is_terminal` host-info leaks. |
| `MCP_V8_HARDEN_REMOVE_BOOTSTRAP` | `true` | Remove `globalThis.__bootstrap` internals (event-loop hooks, pristine primordials). |
| `MCP_V8_HARDEN_REMOVE_SHARED_MEMORY` | `true` | Remove `SharedArrayBuffer` + `Atomics`, the high-resolution Spectre-timer prerequisite. Free in this template: the heap-persistence isolate already disables the WASM threads that would need them. |
| `JWKS_URL` | *(optional, empty)* | Set to an OIDC JWKS endpoint (e.g. Keycloak certs URL) to require JWT bearer auth. Leave unset for an open server. |

Notes:

- Heap persistence uses a V8 `SnapshotCreator` isolate, which disables
  WebAssembly — drop the three heap/fs variables (and the volume) for a
  stateless, WASM-capable deployment. If you do, also drop
  `MCP_V8_HARDEN_REMOVE_SHARED_MEMORY` when the modules need wasm threads
  (emscripten pthreads require `SharedArrayBuffer` + `Atomics`).
- **The sandbox manifest owns outbound egress.** The default deployment dials
  nothing out, so `"mode": "blocked"` costs nothing. Any feature that does
  dial out — `JWKS_URL`, S3 stores, `fetch()` policies, remote OPA,
  `--allow-external-modules`, SSE MCP servers — needs the manifest to open
  the port, typically
  `{"version":"0.1.0","network":{"mode":"blocked","ports":{"connect":[443]}}}`.
  A blocked manifest silently wins over such a feature: the server warns at
  startup and the feature fails at runtime.
- **The OS sandbox fails closed.** Port-scoped Landlock rules (blocked egress
  plus a bind grant for the listener) need Linux 6.7+ with Landlock enabled
  in the runtime's kernel. If Railway's runtime cannot enforce the composed
  set, the deploy aborts with `Landlock not available ...` instead of running
  unconfined — check the deploy logs on a first-boot crash, and only then
  decide whether to remove `MCP_V8_SANDBOX_MANIFEST` and fall back to
  isolate-level sandboxing plus the hardening flags.
- **The default template is unauthenticated.** Scripts run in a sandboxed V8
  isolate with network/filesystem/subprocess access denied by default — and
  with the variables above, ops hardening plus a kernel-enforced deny-all on
  egress underneath it — but anyone with the URL can still burn CPU. For
  anything beyond experiments, set `JWKS_URL`, front the service with an auth
  proxy, or skip the public domain and use
  [private networking](https://docs.railway.com/networking/private-networking)
  only.

### Marketplace overview copy

Ready-to-paste overview for the publish form, following Railway's
[template best practices](https://docs.railway.com/templates/best-practices):

> # Deploy and Host mcp-v8 with Railway
>
> mcp-v8 is a Model Context Protocol server, written in Rust, that gives AI
> agents a sandboxed JavaScript/TypeScript runtime. Instead of dozens of
> narrow tools, the agent gets one `run_js` tool and writes code — looping,
> branching, and transforming data — with durable V8 heap snapshots between
> calls.
>
> ## About Hosting mcp-v8
>
> Hosting mcp-v8 means running a single Rust binary that serves the MCP
> Streamable HTTP endpoint at `/mcp` and a REST sidecar at `/api/exec`. This
> template builds the server from source with Docker, attaches a volume at
> `/data` for heap snapshots, the persistent `/work` filesystem, and the
> session database, and health-checks `/api/version`. Railway injects `PORT`
> automatically; host-header protection is scoped to the service's Railway
> domains. The template deploys sandboxed by default: V8 isolate hardening
> flags are on, and a kernel-enforced (Landlock) OS sandbox restricts the
> process to its own storage paths and blocks outbound network egress.
> Optional JWT auth is one `JWKS_URL` variable away.
>
> ## Common Use Cases
>
> - Give Claude, Cursor, or any MCP client a remote code-execution tool
> - Token-efficient data transformation instead of long tool-call chains
> - Durable agent state via heap snapshots that survive redeploys
> - A REST endpoint for running untrusted JavaScript from your own backend
>
> ## Dependencies for mcp-v8 Hosting
>
> - A volume for persistent heaps, filesystem snapshots, and sessions
> - An MCP client (Claude Code, Claude Desktop, Cursor, ...) or any HTTP client
>
> ### Deployment Dependencies
>
> - [Documentation](https://r33drichards.github.io/mcp-js/)
> - [Source repository](https://github.com/r33drichards/mcp-js)
> - [Model Context Protocol](https://modelcontextprotocol.io)
>
> ### Why Deploy mcp-v8 on Railway?
>
> Railway is a singular platform to deploy your infrastructure stack. Railway
> will host your infrastructure so you don't have to deal with configuration,
> while allowing you to vertically and horizontally scale it.
>
> By deploying mcp-v8 on Railway, you are one step closer to supporting a
> complete full-stack application with minimal burden. Host your servers,
> databases, AI agents, and more on Railway.
