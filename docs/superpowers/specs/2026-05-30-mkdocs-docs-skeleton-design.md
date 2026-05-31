# MkDocs Docs Skeleton Design

## Goal

Create a MkDocs documentation skeleton for this repository that:

- publishes at the repository root URL on GitHub Pages
- includes a landing page at the site root
- keeps public docs separate from internal planning files
- preserves the approved Learn, How-to, Concepts, and Reference structure
- starts with placeholder pages that can be filled in systematically later

## Decision

Use MkDocs instead of Docusaurus.

The public documentation source should live in `site-docs/`, not `docs/`.
This keeps internal files under `docs/superpowers/` out of the published site
by default and gives the public docs a clean, Markdown-first source tree.

## Recommended Stack

- MkDocs
- a root `mkdocs.yml` configuration file
- Markdown files under `site-docs/`
- GitHub Actions for GitHub Pages deployment

The first pass should use plain MkDocs, not Material for MkDocs. The goal is a
simple, stable, text-friendly docs surface with low operational complexity.

## Information Architecture

The documentation model should keep the adapted Django-style split already
approved for this repository.

Top-level navigation:

- Home
- Learn
- How-to
- Concepts
- Reference

The content split remains:

- Learn and How-to for user journeys
- Concepts and Reference for system surfaces

## File Structure

The first scaffold should create:

- `mkdocs.yml`
- `site-docs/index.md`
- `site-docs/learn/overview.md`
- `site-docs/learn/first-run.md`
- `site-docs/learn/transport-walkthrough.md`
- `site-docs/learn/client-walkthrough.md`
- `site-docs/how-to/install-server.md`
- `site-docs/how-to/run-with-stdio.md`
- `site-docs/how-to/run-with-http.md`
- `site-docs/how-to/run-with-sse.md`
- `site-docs/how-to/use-local-storage.md`
- `site-docs/how-to/use-s3-storage.md`
- `site-docs/how-to/enable-opa-policies.md`
- `site-docs/how-to/configure-fetch-header-injection.md`
- `site-docs/how-to/use-rust-client.md`
- `site-docs/concepts/execution-model.md`
- `site-docs/concepts/sessions-and-heaps.md`
- `site-docs/concepts/transports.md`
- `site-docs/concepts/policy-system.md`
- `site-docs/concepts/module-loading.md`
- `site-docs/concepts/clustering.md`
- `site-docs/reference/cli-flags.md`
- `site-docs/reference/http-api.md`
- `site-docs/reference/mcp-tools.md`
- `site-docs/reference/configuration-and-environment.md`
- `site-docs/reference/rust-client-api.md`
- `site-docs/reference/policy-files.md`
- `.github/workflows/deploy-docs.yml`

## Landing Page

The landing page should live at `site-docs/index.md` and render at the site
root.

The first version should be simple and structural, not marketing-heavy. It
should include:

- a short description of `mcp-v8`
- a brief explanation of the documentation layout
- clear links into Learn, How-to, Concepts, and Reference

It should behave as the entry point for both humans and agents.

## Navigation

`mkdocs.yml` should define the navigation explicitly. The first pass should not
rely on implicit file discovery.

The nav should map directly to the approved structure:

- Home
- Learn
  - Overview
  - First Run
  - Transport Walkthrough
  - Client Walkthrough
- How-to
  - Install the Server
  - Run with stdio
  - Run with HTTP
  - Run with SSE
  - Use Local Storage
  - Use S3 Storage
  - Enable OPA Policies
  - Configure Fetch Header Injection
  - Use Rust Client
- Concepts
  - Execution Model
  - Sessions and Heaps
  - Transports
  - Policy System
  - Module Loading
  - Clustering
- Reference
  - CLI Flags
  - HTTP API
  - MCP Tools
  - Configuration and Environment
  - Rust Client API
  - Policy Files

## Placeholder Page Rules

Each placeholder page should:

- have a clear title
- state the intended role of the page
- state what kind of material belongs there
- stay concise
- avoid fake detail and partial implementation guidance

The placeholder text should reflect the document type:

- Learn pages should stay onboarding-oriented
- How-to pages should stay task-oriented
- Concepts pages should explain system ideas and tradeoffs
- Reference pages should stay tightly scoped to interfaces and behavior

## Mapping Strategy

This scaffold should prepare for later migration without migrating existing
content in the same step.

Expected future mapping:

- `README.md` feeds Home and Learn
- `docs/http-api-and-client.md` feeds How-to and Reference
- `tutorials/` feeds Learn or How-to depending on document type
- source-adjacent markdown under `server/src/` feeds Concepts and Reference

## Deployment

GitHub Pages deployment should build the MkDocs site and publish the generated
`site/` output.

The deployment workflow should:

- install Python and MkDocs
- build from the repository root using `mkdocs build`
- upload the generated `site/` directory to GitHub Pages

## Constraints

- The first pass is a skeleton only.
- Existing internal docs under `docs/` should remain untouched.
- Existing public-facing content should not be fully migrated yet.
- The first pass should optimize for simple editing and stable structure.
- The published docs should remain plain-text friendly and easy to parse from
  the repository source.

## Non-Goals

- full documentation migration
- autogenerated API reference
- versioned docs in the first pass
- blog, changelog, or news sections
- theme-heavy customization
- moving internal planning documents into the public docs tree

## Validation

The scaffold is successful if:

- `mkdocs build` succeeds
- the landing page renders at the site root
- the nav matches the approved section structure
- each planned page exists as a placeholder Markdown file
- internal `docs/superpowers/` files are not part of the public site
- the structure is ready for systematic content migration without another IA
  redesign
