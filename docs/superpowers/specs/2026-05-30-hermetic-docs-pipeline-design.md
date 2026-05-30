# Hermetic Docs Pipeline Design

## Goal

Make the MkDocs site build hermetically with Nix, gate every pull request on a
browser-visible end-to-end docs test, and reuse the same Nix build output for
GitHub Pages deployment on every push to `main`.

## Current State

The docs site already exists on `mkdocs-docs-spec`:

- source lives in `site-docs/`
- `mkdocs.yml` defines the nav and output path
- `deploy-docs.yml` deploys on pushes to `main`
- `docs-check.yml` runs `mkdocs build --strict` on pull requests

Today both workflows install MkDocs with Python `pip`. That works, but it is
not hermetic, it does not reuse the repo's Nix infrastructure, and the PR gate
only proves that Markdown compiles. It does not prove that a browser can load
the built site and that the expected content is visible.

The repo already has the right foundation for a Nix-native solution:

- a `flake.nix`
- existing `pkgs.nixosTest` integration tests under `tests/nixos/`
- a GitHub Actions workflow that runs NixOS tests on pull requests and pushes

## Requirements

The new pipeline must satisfy these requirements:

1. The docs build must be defined in Nix and be the source of truth.
2. Every pull request must run a docs gate.
3. That gate must verify not only that the site builds, but that a browser can
   load key pages and that expected content is visible.
4. GitHub Pages deployment on `main` must use the same Nix docs build output.
5. The design must fit the repo's existing NixOS integration-test pattern.

## Recommended Approach

Use one Nix-defined docs package and build everything else around it.

### 1. Add a Nix docs package

Add a flake package for the MkDocs site, for example:

- `packages.docs`

This derivation should:

- take the repo root as input
- install the exact tools needed to build the site
- run the docs generation steps that produce committed reference pages
- run `mkdocs build --strict`
- publish the built site as the derivation output

This becomes the canonical docs artifact.

The key point is that the Pages workflow and the PR gate both consume this same
artifact. No separate Python installer path remains.

### 2. Add a NixOS browser test for docs

Add a new NixOS integration test, for example:

- `checks.x86_64-linux.docs-browser-test`
- file under `tests/nixos/docs-browser.nix`

That test should:

- build the docs package first
- boot a NixOS VM
- serve the built site from a simple static HTTP server inside the VM
- start a browser session
- load key docs pages
- assert that visible text appears on the page

At minimum, check:

- `/` loads and shows the `mcp-v8` home page
- `/quick-start/overview/` loads and shows `Quick Start Overview`
- `/reference/http-api/` loads and shows `HTTP API`
- `/reference/mcp-tools/` loads and shows `MCP Tools`

Because the user asked for a Chrome-based visible-content test, the browser
step should use Chromium or Chrome in the VM and validate rendered page text,
not just HTTP responses.

### 3. Use OCR only where DOM checks are insufficient

The browser test should prefer deterministic visible-text assertions from the
rendered page or browser automation APIs.

OCR should be treated as a fallback, not the primary assertion mechanism.

Reason:

- DOM or browser-visible text checks are more stable
- OCR adds fragility and noise
- the user requirement is about visible rendered content, not image-processing
  for its own sake

If an OCR tool is needed to satisfy a specific browser-only assertion, keep it
scoped to that case.

### 4. Make `docs-check.yml` the PR gate

Change `docs-check.yml` so it runs on every pull request against `main`, not
only when selected docs paths change.

That workflow should:

- install Nix with the features needed for `nixosTest`
- build `.#packages.x86_64-linux.docs` or the equivalent docs package
- run `.#checks.x86_64-linux.docs-browser-test`

This makes the docs gate a real PR gate:

- hermetic build
- browser-visible verification
- same build path as deploy

### 5. Make `deploy-docs.yml` consume the Nix artifact

Change `deploy-docs.yml` so it does not install MkDocs with `pip`.

Instead it should:

- install Nix
- build the docs package
- copy or expose the built site directory from the derivation output
- upload that directory to GitHub Pages

This gives `main` pushes the exact same docs artifact that PR CI validated.

## Workflow Shape

After the change, the flow is:

### Pull requests

`docs-check.yml`

- build docs with Nix
- run the NixOS browser test
- fail if the site does not build or if key content is not visible

### Pushes to `main`

`deploy-docs.yml`

- build docs with the same Nix target
- upload the built site
- deploy to GitHub Pages

### Generated reference drift

`openapi-drift.yml`

- remains responsible for generated reference drift
- should stay separate from the Pages deploy
- may optionally be updated to reuse the same docs package inputs, but it does
  not need to own deployment

## File-Level Design

Expected changes:

- `flake.nix`
  - add a docs package
  - add a docs browser test check
- `tests/nixos/docs-browser.nix`
  - new NixOS integration test for browser-visible docs assertions
- `.github/workflows/docs-check.yml`
  - switch from `pip`-installed MkDocs to Nix build + NixOS test
  - run on every PR to `main`
- `.github/workflows/deploy-docs.yml`
  - switch from `pip`-installed MkDocs to the Nix docs package
- possibly `nix/` helpers
  - only if the docs package needs a small reusable helper derivation

## Testing Strategy

Implementation is complete only if all of these are true:

1. `nix build .#packages.x86_64-linux.docs` succeeds locally.
2. `nix build .#checks.x86_64-linux.docs-browser-test` succeeds locally.
3. The docs browser test proves rendered content is visible on key pages.
4. `docs-check.yml` uses the Nix path and no longer uses `pip install mkdocs`.
5. `deploy-docs.yml` uses the same Nix docs build output.

## Tradeoffs

### Benefits

- one source of truth for docs builds
- hermetic PR gate
- deploy artifact matches what CI validated
- better confidence than static build-only checks

### Costs

- slower CI than a plain `mkdocs build`
- more workflow complexity
- browser tests can be more brittle if assertions are too broad

The design manages that cost by keeping the browser assertions narrow and by
reusing the repo's existing NixOS test pattern instead of inventing a second
test framework.

## Non-Goals

This change does not attempt to:

- add per-PR preview deployments
- replace all repo CI with Nix
- move release workflows onto the docs pipeline
- add visual regression snapshots

## Recommendation

Implement the full Nix path now:

- Nix docs package
- PR-gated NixOS browser test
- Pages deploy from the same Nix artifact

That is the smallest design that satisfies the user's actual requirement:
every PR is gated by a browser-visible docs check, and every push to `main`
deploys the exact same hermetic build.
