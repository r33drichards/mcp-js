# MkDocs README Concepts Expansion Design

**Goal:** Expand the published MkDocs site so the major feature areas already
called out in `README.md` each have a clear conceptual home, with deeper
explanations of behavior, boundaries, and relationships between features.

## Scope

This pass is documentation-only. It does not change runtime behavior, CLI
interfaces, APIs, or deployment architecture.

The expansion should stay within the current MkDocs structure:

- `Home` remains a short landing page
- `Learn` remains onboarding-oriented
- `How-to` remains task-oriented
- `Reference` remains interface-oriented
- `Concepts` becomes the main surface for feature-depth

## Why This Pass Exists

The current MkDocs site has the right top-level structure, but the first-pass
content is intentionally thin. The README already contains a much broader
feature inventory:

- async execution and execution lifecycle
- console output and output pagination
- TypeScript and ES module loading
- WebAssembly support
- content-addressed heap snapshots
- stateless and stateful execution
- multiple transports
- clustering
- concurrency control
- policy-gated fetch and filesystem access
- fetch header injection
- integrations and client surfaces

Right now, many of those features are only lightly represented in the public
docs. The next pass should move that README knowledge into durable concept
pages rather than leaving it trapped in a long README or scattering it into
task pages.

## Documentation Model

This pass keeps the Django-style split already chosen for the docs:

- `Learn` explains how to get oriented and make first progress
- `How-to` solves concrete tasks
- `Concepts` explains system behavior and feature models
- `Reference` documents exact interfaces, flags, endpoints, and schemas

The design principle for this pass is:

**each README feature should map to a concept page unless it is purely a task
or purely a reference surface**

That does not mean one feature always requires one page. Related features
should be grouped when they share a mental model and would read better
together.

## Proposed Concept Structure

The existing `Concepts` section should be expanded into the primary feature
explanation area.

### 1. Execution Model

This page should grow into the conceptual home for:

- async execution lifecycle
- execution IDs and polling model
- promises and top-level `await`
- console output capture and streaming
- cancellation behavior
- concurrency limits and scheduling pressure

This page should explain what happens after code is submitted, how stateful and
stateless execution differ in timing and return shape, and why output is a
separate retrieval surface.

### 2. Sessions and Heaps

This page should remain the home for state persistence and become deeper about:

- content-addressed heap snapshots
- immutable snapshot behavior
- heap restoration
- stateless versus stateful tradeoffs
- named sessions
- session logging
- heap tags and searchability

This is the main page for understanding how long-lived state works and why the
system uses heap hashes rather than mutable server-side sessions.

### 3. Transports

This page should cover:

- stdio transport
- Streamable HTTP transport
- SSE transport
- MCP surface versus plain HTTP API
- when transport choice changes operations or deployment shape

This page should explain that transport changes the client connection model,
not the core execution engine.

### 4. Module Loading

This page should expand to cover:

- ES module execution model
- npm, JSR, and URL imports
- runtime fetch from `esm.sh`
- relative import resolution for fetched modules
- TypeScript stripping
- what “TypeScript support” means and does not mean

This page should make clear that TypeScript support is type stripping, not type
checking, and that external imports are networked runtime resolution rather
than local package-manager installs.

### 5. WASM and Native Modules

This should be a new concept page.

It should explain:

- the WebAssembly runtime model inside `mcp-v8`
- preloaded WASM globals
- config-based versus CLI-based module loading
- WASM memory limits versus V8 heap limits
- the SQLite WASM pattern as a motivating example

This page is concept-first, not a build tutorial.

### 6. Policy System

This page should deepen into the model behind gated capabilities:

- policy chains
- `all` versus `any` evaluation modes
- local Rego versus remote OPA evaluation
- operation namespaces
- default rule and policy path behavior
- why optional capabilities are policy-gated instead of always available

This page should be the umbrella concept page for policy evaluation itself.

### 7. Network Access

This should be a new concept page focused on network behavior.

It should explain:

- how `fetch()` becomes available
- fetch request policy inputs
- response shape and supported surface
- header injection model
- static header rules
- OAuth client-credentials token acquisition and reuse
- precedence between injected headers and user-supplied headers

This page is separate from `Policy System` because users need a focused mental
model for network behavior, not just policy plumbing.

### 8. Filesystem Access

This should be a new concept page.

It should explain:

- when the `fs` module exists
- what kinds of operations it exposes
- the policy input model for filesystem actions
- why filesystem access is optional and gated

This page should explain capability shape and risk boundaries, not step-by-step
setup.

### 9. Clustering

This page should stay concept-focused and become deeper about:

- Raft-inspired coordination
- leader election
- replicated session logging
- write forwarding
- transport constraints
- operational tradeoffs of clustered deployment

This page should help readers understand when clustering changes the system’s
behavior and why it exists.

## Navigation Impact

The top-level nav should remain:

- Home
- Learn
- How-to
- Concepts
- Reference

Within `Concepts`, add the new grouped pages rather than introducing a separate
`Features` section.

Recommended `Concepts` nav after this pass:

- Execution Model
- Sessions and Heaps
- Transports
- Module Loading
- WASM and Native Modules
- Policy System
- Network Access
- Filesystem Access
- Clustering

## Content Sourcing

This pass should be grounded in current repo sources, not invention.

Primary sources:

- `README.md`
- `docs/http-api-and-client.md`
- tutorials under `tutorials/`
- tool-description markdown embedded under `server/src/`
- engine and server implementation files under `server/src/`
- tests that encode expected behavior under `server/tests/`
- client crate docs under `mcp-v8-client/`

The README should act as the feature inventory. The code and tests should act
as the behavior source of truth.

## Writing Rules

Each concept page should:

- explain what the feature is
- explain how it behaves
- explain why it matters
- explain important boundaries and non-goals
- link to how-to or reference pages when the reader needs procedures or exact
  interfaces

Each concept page should avoid:

- turning into a CLI reference
- turning into a recipe page
- repeating large chunks of README text
- making claims not supported by code, tests, or existing docs

## Cross-Linking Model

The concept pages should become the central explanatory layer.

Typical link flow:

- `Concepts` -> `How-to` for setup or execution tasks
- `Concepts` -> `Reference` for exact flags, endpoint lists, and schemas
- `Learn` -> `Concepts` when onboarding reaches deeper understanding

This keeps duplication down while making the docs easier to navigate by intent.

## Implementation Shape

This should be done as a structured docs-content pass, not a nav redesign.

Expected work:

- update `mkdocs.yml` nav only if new concept pages are added
- expand existing concept pages substantially
- add the new concept pages where needed
- add cross-links from current `Learn` and `How-to` pages where they help
- leave the task and reference taxonomy intact

## Out of Scope

This pass should not:

- redesign the whole docs IA again
- add a new top-level `Features` section
- rewrite the public docs into tutorials
- document every CLI flag inline on concept pages
- change runtime code

## Success Criteria

This pass succeeds if:

- the major README features are no longer explained only in `README.md`
- each feature area has a clear concept-level home
- the concept pages read as explanations, not checklists or API dumps
- procedural detail stays mostly in `How-to`
- exact interface detail stays mostly in `Reference`
- the site remains coherent under the current MkDocs nav
