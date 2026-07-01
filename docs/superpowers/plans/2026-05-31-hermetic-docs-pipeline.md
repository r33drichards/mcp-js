# Hermetic Docs Pipeline Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the MkDocs site hermetically with Nix, gate every pull request with a browser-visible NixOS docs test, and deploy GitHub Pages from the same Nix docs artifact on every push to `main`.

**Architecture:** Add a flake `packages.docs` derivation that owns both generated reference pages and the final MkDocs build. Add a `checks.x86_64-linux.docs-browser-test` NixOS VM test that serves the built site and verifies visible page content in Chromium. Rewire `docs-check.yml`, `deploy-docs.yml`, and `openapi-drift.yml` to consume the same Nix build path so generated docs and deployed docs cannot drift.

**Tech Stack:** Nix flakes, `pkgs.nixosTest`, MkDocs, GitHub Actions, Python static HTTP serving, Chromium browser automation inside NixOS test VM, existing Rust/Python docs generators.

---

## File Structure

- Modify: `flake.nix`
  - Add the hermetic docs package and docs browser check.
- Create: `tests/nixos/docs-browser.nix`
  - New browser-visible docs integration test.
- Modify: `.github/workflows/docs-check.yml`
  - Make docs CI run on every PR and use Nix + the docs browser test.
- Modify: `.github/workflows/deploy-docs.yml`
  - Build docs via Nix and upload the resulting artifact to GitHub Pages.
- Modify: `.github/workflows/openapi-drift.yml`
  - Reuse the Nix docs build for generated-reference drift detection.
- Optionally create: `nix/docs.nix`
  - Only if the docs derivation becomes too large for `flake.nix`.

### Task 1: Add the Nix Docs Package

**Files:**
- Modify: `flake.nix`
- Optional Create: `nix/docs.nix`

- [ ] **Step 1: Add a docs package target to the flake**

Add a new package entry alongside `packages.default`:

```nix
packages.docs = pkgs.stdenv.mkDerivation {
  pname = "mcp-js-docs";
  version = "0.1.0";
  src = self;
  nativeBuildInputs = [
    pkgs.python3
    pkgs.python3Packages.mkdocs
    pkgs.python3Packages.mkdocs-mermaid2-plugin
    rustToolchain
    pkgs.clang
    pkgs.llvmPackages.bintools
    pkgs.pkg-config
  ];
  # buildPhase to be filled in next steps
};
```

- [ ] **Step 2: Make the derivation generate the committed reference pages**

Inside the docs package build phase, run the existing source-of-truth generators:

```bash
cargo build --release -p server --bin generate-cli-markdown --bin generate-mcp-tools-markdown --bin server
./target/release/server --print-openapi > openapi.json
cp openapi.json mcp-v8-client/openapi.json
python3 scripts/generate_http_api_reference.py
./target/release/generate-cli-markdown > site-docs/reference/cli-flags.md
./target/release/generate-mcp-tools-markdown > site-docs/reference/mcp-tools.md
```

Expected result: the derivation owns `http-api.md`, `cli-flags.md`, and `mcp-tools.md`.

- [ ] **Step 3: Build the MkDocs site inside the same derivation**

Add the site build and output copy:

```bash
python3 -m mkdocs build --strict
cp -R site $out
```

Expected result: `nix build .#packages.x86_64-linux.docs` produces a static site tree as the derivation output.

- [ ] **Step 4: Keep the derivation hermetic**

Set the same V8 and libclang environment expected by the server build:

```nix
LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
RUSTY_V8_ARCHIVE = "${rustyV8Archive}";
```

Expected result: the docs package can build helper binaries without network downloads during the derivation build.

- [ ] **Step 5: Verify the docs package locally**

Run:

```bash
nix build .#packages.x86_64-linux.docs -L
```

Expected: successful build with an output directory containing `index.html` and generated docs pages under the built site.

- [ ] **Step 6: Commit**

```bash
git add flake.nix nix/docs.nix
git commit -m "build: add hermetic docs package"
```

### Task 2: Add the NixOS Browser Docs Test

**Files:**
- Create: `tests/nixos/docs-browser.nix`
- Modify: `flake.nix`

- [ ] **Step 1: Register the new NixOS test in the flake**

Add a check entry under `checks.x86_64-linux`:

```nix
docs-browser-test = pkgs.nixosTest (import ./tests/nixos/docs-browser.nix {
  inherit pkgs;
  docs-site = self.packages.x86_64-linux.docs;
});
```

Expected result: the flake exposes `.#checks.x86_64-linux.docs-browser-test`.

- [ ] **Step 2: Create a VM that serves the built docs artifact**

In `tests/nixos/docs-browser.nix`, define one node that serves the Nix-built site:

```nix
{ pkgs, docs-site, ... }:
{
  name = "mcp-js-docs-browser";
  nodes.machine = { ... }: {
    networking.firewall.allowedTCPPorts = [ 8000 ];
    environment.systemPackages = [
      pkgs.chromium
      pkgs.python3
    ];
  };
}
```

And in the test script:

```python
machine.succeed("python3 -m http.server 8000 --directory ${docs-site} >/tmp/docs-http.log 2>&1 &")
machine.wait_for_open_port(8000)
```

- [ ] **Step 3: Add browser-visible page assertions**

Use Chromium in headless mode to load the rendered pages and dump page text for assertions:

```python
def page_text(path):
    url = "http://localhost:8000" + path
    return machine.succeed(
        "chromium --headless --disable-gpu --dump-dom " + shlex.quote(url)
    )

home = page_text("/")
assert "mcp-v8" in home
assert "Quick Start" in home

quick_start = page_text("/quick-start/overview/")
assert "Quick Start Overview" in quick_start

http_api = page_text("/reference/http-api/")
assert "HTTP API" in http_api

mcp_tools = page_text("/reference/mcp-tools/")
assert "MCP Tools" in mcp_tools
```

If DOM dumping is insufficient for one page, add a narrowly scoped screenshot + OCR fallback in the same test.

- [ ] **Step 4: Verify the NixOS test locally**

Run:

```bash
nix build .#checks.x86_64-linux.docs-browser-test -L
```

Expected: the VM boots, Chromium loads the site, and assertions pass.

- [ ] **Step 5: Commit**

```bash
git add flake.nix tests/nixos/docs-browser.nix
git commit -m "test: add browser docs nixos check"
```

### Task 3: Rewire Pull Request Docs CI

**Files:**
- Modify: `.github/workflows/docs-check.yml`

- [ ] **Step 1: Make the workflow run on every PR to `main`**

Replace the current path-filter trigger with:

```yaml
on:
  pull_request:
    branches: [ main ]
  workflow_dispatch:
```

Expected result: every PR receives the docs gate, even if the changed files are outside the current path filter.

- [ ] **Step 2: Switch the workflow from pip-installed MkDocs to Nix**

Replace the existing Python-only steps with Nix installation and NixOS-test prerequisites:

```yaml
- uses: actions/checkout@v4
- uses: DeterminateSystems/nix-installer-action@main
  with:
    extra-conf: |
      system-features = nixos-test benchmark big-parallel kvm
      sandbox = false
- name: Enable KVM
  run: |
    echo 'KERNEL=="kvm", GROUP="kvm", MODE="0666", OPTIONS+="static_node=kvm"' | sudo tee /etc/udev/rules.d/99-kvm4all.rules
    sudo udevadm control --reload-rules
    sudo udevadm trigger --name-match=kvm
```

- [ ] **Step 3: Run both the hermetic docs build and browser test**

Add:

```yaml
- name: Build docs package
  run: nix build .#packages.x86_64-linux.docs --print-build-logs --no-link -L

- name: Run docs browser test
  run: nix build .#checks.x86_64-linux.docs-browser-test --print-build-logs --no-link -L
```

Expected result: PR CI proves generated docs, rendered docs, and browser-visible content in one workflow.

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/docs-check.yml
git commit -m "ci: gate pull requests on hermetic docs test"
```

### Task 4: Rewire GitHub Pages Deploy to the Same Nix Build

**Files:**
- Modify: `.github/workflows/deploy-docs.yml`

- [ ] **Step 1: Replace the Python installer path with Nix**

Remove:

```yaml
- uses: actions/setup-python@v5
- run: python -m pip install --upgrade pip
- run: python -m pip install mkdocs mkdocs-mermaid2-plugin
- run: python -m mkdocs build --strict
```

Add:

```yaml
- uses: DeterminateSystems/nix-installer-action@main
- name: Build docs artifact
  run: nix build .#packages.x86_64-linux.docs --print-build-logs -L
```

- [ ] **Step 2: Upload the Nix-built site artifact to Pages**

Use the Nix build output path instead of the mutable `site/` directory:

```yaml
- uses: actions/upload-pages-artifact@v3
  with:
    path: result
```

If the output is not linked as `result`, copy it first:

```yaml
- run: cp -R result site-artifact
```

and upload `site-artifact`.

- [ ] **Step 3: Verify trigger behavior stays main-only**

Keep:

```yaml
on:
  push:
    branches: [main]
```

Expected result: every push to `main` deploys the same Nix artifact the PR gate validated.

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/deploy-docs.yml
git commit -m "ci: deploy pages from nix docs build"
```

### Task 5: Unify Generated Docs Drift Checking

**Files:**
- Modify: `.github/workflows/openapi-drift.yml`

- [ ] **Step 1: Remove the separate ad hoc generation path**

Replace the current sequence:

```yaml
nix develop --command bash -c "cargo build --release -p server"
./target/release/server --print-openapi > openapi.json
python3 scripts/generate_http_api_reference.py
./target/release/generate-cli-markdown > site-docs/reference/cli-flags.md
./target/release/generate-mcp-tools-markdown > site-docs/reference/mcp-tools.md
```

with a Nix docs build that reuses the same generation logic:

```yaml
nix build .#packages.x86_64-linux.docs --print-build-logs -L
```

- [ ] **Step 2: Keep drift detection focused on committed generated sources**

After the Nix build, diff only the committed generated artifacts:

```yaml
if ! git diff --exit-code openapi.json mcp-v8-client/openapi.json site-docs/reference/http-api.md site-docs/reference/cli-flags.md site-docs/reference/mcp-tools.md; then
  exit 1
fi
```

Expected result: there is still an explicit stale-generated-files check, but it now depends on the same hermetic generation path as the docs package.

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/openapi-drift.yml
git commit -m "ci: reuse nix docs build for generated docs drift"
```

### Task 6: Full Verification and Integration

**Files:**
- Verify only

- [ ] **Step 1: Run local hermetic docs build**

Run:

```bash
nix build .#packages.x86_64-linux.docs -L
```

Expected: success; generated reference pages and final site are built from the same derivation.

- [ ] **Step 2: Run local browser-visible docs VM test**

Run:

```bash
nix build .#checks.x86_64-linux.docs-browser-test -L
```

Expected: success; Chromium loads docs pages and visible text assertions pass.

- [ ] **Step 3: Re-run generated-reference verification**

Run:

```bash
git diff --exit-code openapi.json mcp-v8-client/openapi.json site-docs/reference/http-api.md site-docs/reference/cli-flags.md site-docs/reference/mcp-tools.md
```

Expected: no diff.

- [ ] **Step 4: Dry-run the workflow logic locally where possible**

Run:

```bash
nix build .#packages.x86_64-linux.docs -L
/home/node/.local/bin/mkdocs build --strict
```

Expected: both succeed; the MkDocs build remains consistent with the Nix artifact.

- [ ] **Step 5: Push the branch**

```bash
git push origin mkdocs-docs-spec
```

- [ ] **Step 6: Commit**

```bash
git status --short
```

Expected: clean worktree after push and final review.
