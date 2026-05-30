# Docs Iteration Tracker

This file tracks the ongoing MkDocs documentation work on `mkdocs-docs-spec`
so follow-up tasks can be handled incrementally.

## Current branch

- branch: `mkdocs-docs-spec`
- PR: `#155`

## Completed

- switched the docs site from Docusaurus planning to MkDocs
- rewrote the home page introduction to explain `mcp-v8` as a lightweight,
  policy-enforced sandbox for agent compute with lower overhead than VM-based
  sandbox products
- created top-level public sections for:
  - `Quick Start`
  - `Install`
  - `How-to`
  - `Concepts`
  - `Reference`
- added overview pages for:
  - `Quick Start`
  - `Install`
  - `How-to`
  - `Concepts`
  - `Reference`
- removed the old `Learn` section from the public nav
- split `Quick Start` into separate pages for:
  - Claude Code
  - Codex
  - Cursor
  - generic MCP
  - `curl`
  - bundled CLI
- promoted `Install` to a top-level section with separate pages for:
  - release script
  - GitHub releases
  - build from source
  - Nix
- expanded concept and how-to coverage for:
  - execution model
  - sessions and heaps
  - transports
  - integration surfaces
  - module loading
  - WASM and native modules
  - policy system
  - network access
  - filesystem access
  - clustering
- covered both local stdio MCP configuration and remote HTTP MCP configuration
  in the public setup docs
- generated the MCP tools reference page from the built-in MCP tool registry
- generated the CLI flags reference page from the Clap `Cli` definition
- added Mermaid support and diagrams
- added docs CI for `mkdocs build --strict`
- generated the HTTP API reference from `openapi.json`
- removed committed `site/` build output from version control and ignored it
- carried the major README feature areas into public docs pages for:
  - quick start and installation
  - stateful vs stateless execution
  - transports and MCP integration
  - HTTP API, MCP tools, and CLI flags
  - WASM, fetch, filesystem access, storage, and clustering

## In progress

None.

## Next tasks

None right now.

## Notes

- update this file at the end of each meaningful docs pass
- prefer linking existing pages rather than repeating the same setup steps in multiple places
