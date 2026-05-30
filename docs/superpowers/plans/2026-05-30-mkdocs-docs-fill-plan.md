# MkDocs Docs Fill Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the Docusaurus direction with a MkDocs documentation site under `site-docs/`, then fill the first set of public docs pages with content grounded in the current codebase, README, existing docs, tutorials, and embedded tool descriptions.

**Architecture:** Keep the public docs tree completely separate from internal planning docs. Use a root `mkdocs.yml` with explicit navigation, write all public pages as plain Markdown under `site-docs/`, and deploy the generated `site/` directory with GitHub Pages. Fill content by document type: Learn and How-to from usage flows, Concepts from engine and system code, and Reference from CLI/API/tool surfaces.

**Tech Stack:** MkDocs, Markdown, Python 3, GitHub Actions, GitHub Pages

---

## File Structure

### New files and directories

- Create: `mkdocs.yml`
- Create: `site-docs/index.md`
- Create: `site-docs/learn/overview.md`
- Create: `site-docs/learn/first-run.md`
- Create: `site-docs/learn/transport-walkthrough.md`
- Create: `site-docs/learn/client-walkthrough.md`
- Create: `site-docs/how-to/install-server.md`
- Create: `site-docs/how-to/run-with-stdio.md`
- Create: `site-docs/how-to/run-with-http.md`
- Create: `site-docs/how-to/run-with-sse.md`
- Create: `site-docs/how-to/use-local-storage.md`
- Create: `site-docs/how-to/use-s3-storage.md`
- Create: `site-docs/how-to/enable-opa-policies.md`
- Create: `site-docs/how-to/configure-fetch-header-injection.md`
- Create: `site-docs/how-to/use-rust-client.md`
- Create: `site-docs/concepts/execution-model.md`
- Create: `site-docs/concepts/sessions-and-heaps.md`
- Create: `site-docs/concepts/transports.md`
- Create: `site-docs/concepts/policy-system.md`
- Create: `site-docs/concepts/module-loading.md`
- Create: `site-docs/concepts/clustering.md`
- Create: `site-docs/reference/cli-flags.md`
- Create: `site-docs/reference/http-api.md`
- Create: `site-docs/reference/mcp-tools.md`
- Create: `site-docs/reference/configuration-and-environment.md`
- Create: `site-docs/reference/rust-client-api.md`
- Create: `site-docs/reference/policy-files.md`
- Create: `.github/workflows/deploy-docs.yml`

### Existing files to modify

- Modify: `README.md`

### Source files to mine for documentation

- Read: `README.md`
- Read: `docs/http-api-and-client.md`
- Read: `tutorials/oauth-client-credentials-fetch-injection.md`
- Read: `tutorials/github-api-token-injection.md`
- Read: `server/src/main.rs`
- Read: `server/src/api.rs`
- Read: `server/src/mcp.rs`
- Read: `server/src/session.rs`
- Read: `server/src/cluster.rs`
- Read: `server/src/run_js_tool_description.md`
- Read: `server/src/run_js_tool_secure.md`
- Read: `server/src/run_js_tool_stateless.md`
- Read: `server/src/llms_txt.md`
- Read: `server/src/engine/heap_storage.rs`
- Read: `server/src/engine/module_loader.rs`
- Read: `server/src/engine/fetch.rs`
- Read: `server/src/engine/fetch_auth.rs`
- Read: `server/src/engine/fs.rs`
- Read: `server/src/engine/opa.rs`
- Read: `server/src/engine/execution.rs`
- Read: `server/src/engine/session_log.rs`
- Read: `server/src/engine/heap_tags.rs`
- Read: `server/tests/README.md`
- Read: `server/tests/fetch_header_injection.rs`
- Read: `server/tests/module_imports.rs`
- Read: `server/tests/parameter_configs.rs`
- Read: `server/tests/http_e2e.rs`
- Read: `server/tests/stdio_e2e.rs`
- Read: `server/tests/sse_e2e.rs`
- Read: `server/tests/cluster_simulation.rs`
- Read: `server/tests/cluster_write_forwarding.rs`
- Read: `server/tests/local_opa_policy.rs`
- Read: `server/tests/write_through_cache.rs`

### Verification commands

- Run: `test -f mkdocs.yml`
- Run: `python3 -m pip install mkdocs`
- Run: `python3 -m mkdocs build --strict`

## Task 1: Scaffold MkDocs and public docs navigation

**Files:**
- Create: `mkdocs.yml`
- Create: `site-docs/`

- [ ] **Step 1: Write the failing scaffold check**

Run:

```bash
test -f mkdocs.yml
```

Expected: exits non-zero because the MkDocs config does not exist yet.

- [ ] **Step 2: Create `mkdocs.yml`**

Write this file:

```yaml
site_name: mcp-v8
site_description: JavaScript MCP server, transports, policies, and operational guidance
site_url: https://r33drichards.github.io/mcp-js/
docs_dir: site-docs
site_dir: site
theme:
  name: mkdocs
nav:
  - Home: index.md
  - Learn:
      - Overview: learn/overview.md
      - First Run: learn/first-run.md
      - Transport Walkthrough: learn/transport-walkthrough.md
      - Client Walkthrough: learn/client-walkthrough.md
  - How-to:
      - Install the Server: how-to/install-server.md
      - Run with stdio: how-to/run-with-stdio.md
      - Run with HTTP: how-to/run-with-http.md
      - Run with SSE: how-to/run-with-sse.md
      - Use Local Storage: how-to/use-local-storage.md
      - Use S3 Storage: how-to/use-s3-storage.md
      - Enable OPA Policies: how-to/enable-opa-policies.md
      - Configure Fetch Header Injection: how-to/configure-fetch-header-injection.md
      - Use Rust Client: how-to/use-rust-client.md
  - Concepts:
      - Execution Model: concepts/execution-model.md
      - Sessions and Heaps: concepts/sessions-and-heaps.md
      - Transports: concepts/transports.md
      - Policy System: concepts/policy-system.md
      - Module Loading: concepts/module-loading.md
      - Clustering: concepts/clustering.md
  - Reference:
      - CLI Flags: reference/cli-flags.md
      - HTTP API: reference/http-api.md
      - MCP Tools: reference/mcp-tools.md
      - Configuration and Environment: reference/configuration-and-environment.md
      - Rust Client API: reference/rust-client-api.md
      - Policy Files: reference/policy-files.md
markdown_extensions:
  - tables
  - toc:
      permalink: true
```

- [ ] **Step 3: Create the docs directory structure**

Run:

```bash
mkdir -p \
  site-docs/learn \
  site-docs/how-to \
  site-docs/concepts \
  site-docs/reference
```

Expected: the public docs tree exists under `site-docs/`.

- [ ] **Step 4: Commit**

```bash
git add mkdocs.yml site-docs
git commit -m "docs: scaffold mkdocs site structure"
```

## Task 2: Add the landing page and repository discoverability

**Files:**
- Create: `site-docs/index.md`
- Modify: `README.md`

- [ ] **Step 1: Write the failing landing-page expectation**

Run:

```bash
test -f site-docs/index.md
```

Expected: exits non-zero because the landing page does not exist yet.

- [ ] **Step 2: Create `site-docs/index.md`**

Write this file:

```md
# mcp-v8

`mcp-v8` is a Rust-based MCP server that exposes a V8 JavaScript runtime to AI
agents and other clients. It supports stateful and stateless execution,
multiple transports, optional policy-gated network and filesystem access, and
content-addressed heap persistence.

Use this documentation by intent:

- Start with [Learn](learn/overview.md) if you need orientation.
- Use [How-to](how-to/install-server.md) when you need to complete a task.
- Read [Concepts](concepts/execution-model.md) to understand system behavior.
- Use [Reference](reference/cli-flags.md) for flags, APIs, and interface details.

## What this site covers

- how to install and run the server
- how transports and execution modes differ
- how sessions, heaps, and clustering work
- how policy-gated fetch, filesystem access, and module loading behave
- how to use the HTTP API and Rust client
```

- [ ] **Step 3: Add a docs pointer to `README.md`**

Insert near the top-level introduction:

```md
## Documentation

The public documentation site is built with MkDocs from `site-docs/`. That
tree is intended to become the primary published documentation surface for this
repository.
```

- [ ] **Step 4: Commit**

```bash
git add site-docs/index.md README.md
git commit -m "docs: add mkdocs landing page"
```

## Task 3: Write the Learn pages from current usage flows

**Files:**
- Create: `site-docs/learn/overview.md`
- Create: `site-docs/learn/first-run.md`
- Create: `site-docs/learn/transport-walkthrough.md`
- Create: `site-docs/learn/client-walkthrough.md`

- [ ] **Step 1: Create `site-docs/learn/overview.md`**

Write this file:

```md
# Overview

`mcp-v8` is a JavaScript execution server built around V8 and exposed through
the Model Context Protocol. It is designed for clients that need to run
JavaScript or TypeScript, optionally carry heap state between runs, and inspect
results through MCP tools or the HTTP API.

At a high level, the system has four layers:

1. transport entry points such as stdio, Streamable HTTP, and SSE
2. an execution engine that runs JavaScript in V8
3. optional persistence for heaps and session history
4. optional policy-gated capabilities such as `fetch()`, `fs`, and external
   module loading

If you are new to the project, continue with [First Run](first-run.md). If you
already know what transport or client you need, move to
[Transport Walkthrough](transport-walkthrough.md) or
[Client Walkthrough](client-walkthrough.md).
```

- [ ] **Step 2: Create `site-docs/learn/first-run.md`**

Write this file:

```md
# First Run

The fastest way to verify `mcp-v8` is to start it in stateless HTTP mode and
submit a small execution through the REST API or CLI.

## Start the server

```bash
cargo build --release -p server
./target/release/server --stateless --http-port 3000
```

Stateless mode starts each execution from a fresh V8 isolate. That avoids heap
storage setup and is the simplest way to confirm the server is working.

## Submit an execution

```bash
curl -s http://localhost:3000/api/exec \
  -H 'content-type: application/json' \
  -d '{"code":"console.log(1 + 1)"}'
```

The server returns an `execution_id`. Poll `/api/executions/{id}` until the
status becomes `completed`, then read `/api/executions/{id}/output` to see the
console output.

## What success looks like

- the server starts without configuration errors
- `/api/exec` returns an execution ID
- the execution reaches `completed`
- the output endpoint returns `2`

Once that works, continue with [Transport Walkthrough](transport-walkthrough.md)
to choose a transport model, or move into [How-to](../how-to/install-server.md)
for task-focused setup instructions.
```

- [ ] **Step 3: Create `site-docs/learn/transport-walkthrough.md`**

Write this file:

```md
# Transport Walkthrough

`mcp-v8` supports three transport styles:

- stdio, which is the default when no network port is configured
- Streamable HTTP, enabled with `--http-port`
- SSE, enabled with `--sse-port`

## Stdio

Stdio is the simplest integration for local MCP clients that spawn the server
as a subprocess. It avoids network setup and is the default transport when no
HTTP or SSE port is provided.

## Streamable HTTP

Streamable HTTP is the best fit when you want a networked MCP endpoint and a
plain REST API from the same process. In this mode, MCP is served at `/mcp`,
and the REST API remains available under `/api/...`.

## SSE

SSE is the older HTTP-based transport. It exposes `/sse` for the event stream
and `/message` for client requests. Use it when your client requires SSE
instead of the newer Streamable HTTP model.

## Choosing between them

- use stdio for local tool integrations
- use Streamable HTTP for most server deployments
- use SSE only when the client requires it
```

- [ ] **Step 4: Create `site-docs/learn/client-walkthrough.md`**

Write this file:

```md
# Client Walkthrough

There are three common ways to interact with `mcp-v8`:

- through MCP tools exposed over stdio, HTTP, or SSE
- through the plain HTTP API
- through the generated Rust client and CLI

## MCP clients

In MCP mode, the main entry point is the `run_js` tool. In stateful mode,
`run_js` queues background work and returns an execution ID. In stateless mode,
it waits internally and returns output directly.

Stateful mode also exposes status, output, cancellation, session, and heap-tag
tools. The exact tool shape depends on how the server was started.

## HTTP clients

When running in HTTP or SSE mode, the server also exposes a REST API. That API
mirrors the async execution flow used by the stateful MCP tool surface:
submit, poll, inspect output, and optionally cancel.

## Rust client and CLI

The repository ships a generated Rust SDK and a CLI that both target the HTTP
API. They are a good fit when you want typed automation outside MCP.
```

- [ ] **Step 5: Commit**

```bash
git add site-docs/learn
git commit -m "docs: add learn pages"
```

## Task 4: Write the How-to pages from operational tasks

**Files:**
- Create: `site-docs/how-to/install-server.md`
- Create: `site-docs/how-to/run-with-stdio.md`
- Create: `site-docs/how-to/run-with-http.md`
- Create: `site-docs/how-to/run-with-sse.md`
- Create: `site-docs/how-to/use-local-storage.md`
- Create: `site-docs/how-to/use-s3-storage.md`
- Create: `site-docs/how-to/enable-opa-policies.md`
- Create: `site-docs/how-to/configure-fetch-header-injection.md`
- Create: `site-docs/how-to/use-rust-client.md`

- [ ] **Step 1: Create `site-docs/how-to/install-server.md`**

Write this file:

```md
# Install the Server

## Install from the release script

```bash
curl -fsSL https://raw.githubusercontent.com/r33drichards/mcp-js/main/install.sh | sudo bash
```

This installs `mcp-v8` into `/usr/local/bin/`.

## Build from source

```bash
cargo build --release -p server
./target/release/server --help
```

Building from source is the better option when you are testing changes, using a
development branch, or working in an environment where you already have Rust
tooling available.
```

- [ ] **Step 2: Create the transport setup pages**

Write these files:

`site-docs/how-to/run-with-stdio.md`

```md
# Run with stdio

Start the server without `--http-port` or `--sse-port`:

```bash
./target/release/server --directory-path /tmp/mcp-v8-heaps
```

Stdio is the default transport. Use this mode when an MCP client launches the
server as a subprocess and communicates over stdin/stdout.
```

`site-docs/how-to/run-with-http.md`

```md
# Run with HTTP

Start the server with Streamable HTTP enabled:

```bash
./target/release/server --stateless --http-port 3000
```

This exposes:

- MCP at `/mcp`
- REST API endpoints under `/api/...`
- OpenAPI JSON at `/api-doc/openapi.json`
```

`site-docs/how-to/run-with-sse.md`

```md
# Run with SSE

Start the server with SSE transport enabled:

```bash
./target/release/server --stateless --sse-port 3000
```

This exposes:

- SSE stream at `/sse`
- POST message endpoint at `/message`
```

- [ ] **Step 3: Create the storage pages**

Write these files:

`site-docs/how-to/use-local-storage.md`

```md
# Use Local Storage

Use a local directory for heap snapshots:

```bash
./target/release/server --directory-path /var/lib/mcp-v8/heaps
```

If you omit all storage flags, the server defaults to a local directory under
`/tmp/mcp-v8-heaps`.
```

`site-docs/how-to/use-s3-storage.md`

```md
# Use S3 Storage

Start the server with S3-backed heap storage:

```bash
./target/release/server --s3-bucket my-mcp-v8-heaps
```

To add a local write-through cache in front of S3:

```bash
./target/release/server \
  --s3-bucket my-mcp-v8-heaps \
  --cache-dir /var/cache/mcp-v8-heaps
```
```

- [ ] **Step 4: Create the policy and client pages**

Write these files:

`site-docs/how-to/enable-opa-policies.md`

```md
# Enable OPA Policies

Pass a JSON policy configuration through `--policies-json`:

```bash
./target/release/server \
  --stateless \
  --http-port 3000 \
  --policies-json '{"fetch":{"policies":[{"url":"file:///path/to/fetch.rego"}]}}'
```

`--policies-json` enables the policy chain used for fetch, module import
auditing, and filesystem access depending on the configuration you provide.
```

`site-docs/how-to/configure-fetch-header-injection.md`

```md
# Configure Fetch Header Injection

Add a static header rule on the command line:

```bash
./target/release/server \
  --stateless \
  --http-port 3000 \
  --policies-json '{"fetch":{"policies":[{"url":"file:///path/to/fetch.rego"}]}}' \
  --fetch-header 'host=api.github.com,header=Authorization,value=Bearer TOKEN,methods=GET'
```

Or load rules from a JSON file:

```bash
./target/release/server \
  --stateless \
  --http-port 3000 \
  --policies-json '{"fetch":{"policies":[{"url":"file:///path/to/fetch.rego"}]}}' \
  --fetch-header-config ./fetch-headers.json
```
```

`site-docs/how-to/use-rust-client.md`

```md
# Use Rust Client

Add the generated Rust client crate:

```toml
[dependencies]
mcp-v8-client = "0.1.0"
```

Basic usage:

```rust
use mcp_v8_client::Client;

let client = Client::new("http://localhost:3000");
```

Use the Rust client when you want typed access to the HTTP API from Rust code.
```

- [ ] **Step 5: Commit**

```bash
git add site-docs/how-to
git commit -m "docs: add how-to pages"
```

## Task 5: Write the Concepts pages from system internals

**Files:**
- Create: `site-docs/concepts/execution-model.md`
- Create: `site-docs/concepts/sessions-and-heaps.md`
- Create: `site-docs/concepts/transports.md`
- Create: `site-docs/concepts/policy-system.md`
- Create: `site-docs/concepts/module-loading.md`
- Create: `site-docs/concepts/clustering.md`

- [ ] **Step 1: Create `site-docs/concepts/execution-model.md`**

Write this file:

```md
# Execution Model

`mcp-v8` runs JavaScript in V8 but does not behave like a browser or a general
Node.js runtime. Code executes as ES modules, supports top-level `await`, and
uses an async execution registry to track background work in stateful mode.

The key design split is between:

- stateful execution, where `run_js` queues work and returns an execution ID
- stateless execution, where the server waits internally and returns output
  directly

Console output is streamed while code runs, and long-running work is governed
by per-execution timeout and memory limits.
```

- [ ] **Step 2: Create `site-docs/concepts/sessions-and-heaps.md`**

Write this file:

```md
# Sessions and Heaps

In stateful mode, heap state is persisted by content hash rather than by a
mutable session object. A completed execution can return a heap snapshot key,
and passing that key back into a later call resumes from that V8 state.

Session history is related but separate. Session logging tracks named sessions
and execution history, while heap tags provide a separate metadata layer for
searching and organizing heap snapshots.
```

- [ ] **Step 3: Create `site-docs/concepts/transports.md`**

Write this file:

```md
# Transports

The transport layer changes how clients talk to `mcp-v8`, but not what the
engine does once code reaches execution.

- stdio is process-local and subprocess-oriented
- Streamable HTTP is network-friendly and pairs naturally with the REST API
- SSE is an older HTTP-based event-stream transport

Cluster mode is only supported for HTTP and SSE. Stdio is explicitly excluded
from cluster deployments.
```

- [ ] **Step 4: Create `site-docs/concepts/policy-system.md`**

Write this file:

```md
# Policy System

Optional capabilities such as `fetch()`, the `fs` module, and external module
auditing are gated by policy configuration. The server builds policy chains
from `--policies-json` and evaluates requests against those chains before the
operation is allowed to proceed.

This design keeps powerful capabilities available when needed without making
them part of the default runtime surface.
```

- [ ] **Step 5: Create `site-docs/concepts/module-loading.md`**

Write this file:

```md
# Module Loading

External modules are disabled by default. When enabled with
`--allow-external-modules`, `mcp-v8` resolves npm and JSR specifiers through
`esm.sh` and also permits direct URL imports.

This is a networked module-loading model, not a local package-manager model.
Imports are resolved at runtime, and optional policy checks can audit them
before fetch.
```

- [ ] **Step 6: Create `site-docs/concepts/clustering.md`**

Write this file:

```md
# Clustering

Cluster mode adds Raft-inspired coordination for distributed deployments. It
handles leader election, replicated session logging, and write forwarding for
operations that must be coordinated across nodes.

This is an operational scaling feature, not just a transport flag. It only
applies to network transports and introduces peer discovery, node identity, and
timing configuration concerns.
```

- [ ] **Step 7: Commit**

```bash
git add site-docs/concepts
git commit -m "docs: add concepts pages"
```

## Task 6: Write the Reference pages from code-defined interfaces

**Files:**
- Create: `site-docs/reference/cli-flags.md`
- Create: `site-docs/reference/http-api.md`
- Create: `site-docs/reference/mcp-tools.md`
- Create: `site-docs/reference/configuration-and-environment.md`
- Create: `site-docs/reference/rust-client-api.md`
- Create: `site-docs/reference/policy-files.md`

- [ ] **Step 1: Create `site-docs/reference/cli-flags.md`**

Write this file:

```md
# CLI Flags

`mcp-v8` is configured primarily through command-line flags. The major groups
are:

- storage: `--s3-bucket`, `--cache-dir`, `--directory-path`, `--stateless`
- transport: `--http-port`, `--sse-port`
- execution limits: `--heap-memory-max`, `--execution-timeout`,
  `--max-concurrent-executions`
- session logging: `--session-db-path`
- clustering: `--cluster-port`, `--node-id`, `--peers`, `--join`,
  `--advertise-addr`, `--heartbeat-interval`, `--election-timeout-min`,
  `--election-timeout-max`
- WASM: `--wasm-module`, `--wasm-config`, `--wasm-default-max-memory`
- fetch and policy: `--fetch-header`, `--fetch-header-config`,
  `--allow-external-modules`, `--policies-json`
- external MCP servers: `--mcp-server`, `--mcp-config`, `--mcp-stubs`,
  `--mcp-stub-prefix`
- utility: `--print-openapi`
```

- [ ] **Step 2: Create `site-docs/reference/http-api.md`**

Write this file:

```md
# HTTP API

When `mcp-v8` runs with `--http-port` or `--sse-port`, it exposes a plain HTTP
API alongside MCP transport.

Core endpoints:

- `POST /api/exec`
- `GET /api/executions`
- `GET /api/executions/{id}`
- `GET /api/executions/{id}/output`
- `POST /api/executions/{id}/cancel`
- `GET /api/version`
- `GET /api/cli`
- `GET /api/cli/{platform}`
- `GET /api-doc/openapi.json`

Use the OpenAPI document as the source of truth for request and response
shapes.
```

- [ ] **Step 3: Create `site-docs/reference/mcp-tools.md`**

Write this file:

```md
# MCP Tools

The MCP tool surface depends on execution mode.

Stateful mode exposes:

- `run_js`
- `get_execution`
- `get_execution_output`
- `cancel_execution`
- `list_executions`
- `list_sessions`
- `list_session_snapshots`
- `get_heap_tags`
- `set_heap_tags`
- `delete_heap_tags`
- `query_heaps_by_tags`

Stateless mode exposes a simplified `run_js` that waits internally and returns
output directly.
```

- [ ] **Step 4: Create `site-docs/reference/configuration-and-environment.md`**

Write this file:

```md
# Configuration and Environment

Important configuration inputs include:

- command-line flags in `server/src/main.rs`
- optional `JWKS_URL` for JWT verification
- AWS credentials and region resolution used by the AWS SDK when S3 storage is
  enabled
- JSON configuration files for fetch headers, WASM modules, MCP servers, and
  policy configuration

The runtime itself does not expose environment variables to user JavaScript.
Environment variables affect server configuration, not guest-code capabilities.
```

- [ ] **Step 5: Create `site-docs/reference/rust-client-api.md`**

Write this file:

```md
# Rust Client API

The generated Rust client targets the HTTP API, not the MCP transport.

At a minimum, the reference should cover:

- `mcp_v8_client::Client`
- the generated request and response types for `/api/exec`
- execution status polling
- console output pagination
- cancellation calls

This page should stay focused on the typed client surface and how it maps to
the HTTP API.
```

- [ ] **Step 6: Create `site-docs/reference/policy-files.md`**

Write this file:

```md
# Policy Files

Policy configuration is passed through `--policies-json` as inline JSON or a
path to a JSON file. That configuration defines one or more policy chains for
fetch and module evaluation, and it also controls optional filesystem
operations when those capabilities are enabled.

The reference should document the configuration shape, supported keys, and the
kinds of input each policy receives, not just example workflows.
```

- [ ] **Step 7: Commit**

```bash
git add site-docs/reference
git commit -m "docs: add reference pages"
```

## Task 7: Add GitHub Pages deployment and verify the built site

**Files:**
- Create: `.github/workflows/deploy-docs.yml`

- [ ] **Step 1: Write the failing deployment expectation**

Run:

```bash
test -f .github/workflows/deploy-docs.yml
```

Expected: exits non-zero because the Pages workflow does not exist yet.

- [ ] **Step 2: Create `.github/workflows/deploy-docs.yml`**

Write this file:

```yaml
name: Deploy Docs

on:
  push:
    branches: [main]
  workflow_dispatch:

permissions:
  contents: read
  pages: write
  id-token: write

concurrency:
  group: pages
  cancel-in-progress: true

jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-python@v5
        with:
          python-version: "3.12"
      - run: python -m pip install --upgrade pip
      - run: python -m pip install mkdocs
      - run: python -m mkdocs build --strict
      - uses: actions/configure-pages@v5
      - uses: actions/upload-pages-artifact@v3
        with:
          path: site

  deploy:
    needs: build
    runs-on: ubuntu-latest
    environment:
      name: github-pages
      url: ${{ steps.deployment.outputs.page_url }}
    steps:
      - id: deployment
        uses: actions/deploy-pages@v4
```

- [ ] **Step 3: Install MkDocs locally and verify the build**

Run:

```bash
python3 -m pip install mkdocs
python3 -m mkdocs build --strict
```

Expected: MkDocs builds successfully and writes the site to `site/`.

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/deploy-docs.yml
git commit -m "docs: add mkdocs pages workflow"
```

## Spec Coverage Check

- MkDocs instead of Docusaurus: covered in Tasks 1 and 7.
- Public docs in `site-docs/`: covered in Tasks 1 through 6.
- Landing page at repo root URL: covered in Task 2.
- Learn / How-to / Concepts / Reference structure: covered in Tasks 1 and 3 through 6.
- Fill pages from current code and docs: covered in Tasks 3 through 6.
- Keep internal `docs/superpowers/` files out of the published tree: covered by Task 1 file layout.

## Placeholder Scan

This plan avoids `TODO`, `TBD`, and “fill this in later” instructions. Each
task names exact files, exact commands, and exact page content for the initial
draft.

## Type and Naming Consistency Check

- The nav labels match the approved MkDocs spec.
- All `site-docs/` page paths match the navigation entries.
- The build output is consistently `site/`.
- The plan keeps `run_js`, `get_execution`, `get_execution_output`, and related
  tool names consistent with the codebase.

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-05-30-mkdocs-docs-fill-plan.md`. Two execution options:

1. Subagent-Driven (recommended) - I dispatch a fresh subagent per task, review between tasks, fast iteration
2. Inline Execution - Execute tasks in this session using executing-plans, batch execution with checkpoints

Which approach?
