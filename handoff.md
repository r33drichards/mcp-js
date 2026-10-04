# npm packaging handoff

## Stop boundary

This PR is preparation only. Do not publish either npm package, create or push a
release tag, create a GitHub release, merge the PR, upload artifacts, or start a
workflow that uploads artifacts unless the repository owner gives renewed,
explicit approval. Do not add npm tokens to the repository or share npm login,
2FA, setup-code, or trusted-publisher links in chat.

The owner most recently said not to upload anything yet. Before that stop was
received, run 34176267690 had already uploaded two GitHub Actions artifacts and
commit `d015af3c` had already been pushed. Treat those as validation evidence,
not as approval of version `0.21.0` or approval to publish. The files under
`bootstrap-artifacts/` in the originating worktree are untracked local outputs;
they are deliberately not part of this PR.

## Objective and package contract

Prepare, validate, and eventually bootstrap these two public packages together:

| Package | Source | License | Runtime support |
| --- | --- | --- | --- |
| `@wholelottahoopla/mcp-js-node` | `node/` | AGPL-3.0-only, with the verbatim root license | Linux x64 with glibc; Node.js 22+ |
| `@wholelottahoopla/mcp-js-client` | `clients/typescript/` | Owner-approved MIT package license | Portable HTTP client; Node.js 18+ |

Both packages follow the repository's existing release cadence. A single strict
`vX.Y.Z` or `vX.Y.Z-prerelease` repository tag supplies the same version to both
packages. Stable versions publish with npm dist-tag `latest`; versions with a
prerelease component publish with `next`. Do not reintroduce package-specific
`node-v*` or `client-v*` tags. Existing Rust and Docker release behavior remains
unchanged.

The checked-in npm manifests currently say `0.1.0`; this is a development
placeholder, not an approved bootstrap version. `node/scripts/release-tag.mjs`
validates a repository tag and stages an owner-approved version into both the
package manifest and the root entries of its lockfile before a build. Select the
actual bootstrap/release version with the owner; do not infer it from old tags,
examples, filenames, or the previous `0.21.0` validation run.

## Checkout

A fresh handoff checkout can be created with:

```sh
git clone https://github.com/r33drichards/mcp-js.git
cd mcp-js
gh pr checkout 259
git rev-parse HEAD
git status --short
```

The expected handoff branch is `openclaw/native-npm-packaging`. Confirm the PR's
current head rather than assuming the SHA in this document remains current.
Work only in an isolated checkout/worktree and preserve unrelated changes.

A source checkout does not contain a generated native `.so`, generated UniFFI
bindings, or npm tarballs. `node/generated/`, `node/dist/`, and package tarballs
are build outputs. The HTTP `dist/` files are also generated. They must be
created and validated; do not mistake a successful checkout for a releasable
artifact.

## Implementation map

- `.github/workflows/npm-publish.yml`: manual validation on protected `main`,
  exact Nix tarball preservation, and environment-gated tag publication. Only
  the publish job has `id-token: write`.
- `.github/workflows/node-uniffi-e2e.yml`: source-built native validation and
  HTTP Nix package validation. At implementation commit `d015af3c` it uploads
  validated tarballs at the end; because uploads are currently forbidden, do not dispatch it until
  the owner explicitly reauthorizes uploads or the upload steps are safely
  changed under owner direction.
- `.github/workflows/npm-client-package-e2e.yml`: focused HTTP consumer checks.
- `node/scripts/release-tag.mjs`: strict shared `v...` parsing and package plus
  lockfile version staging.
- `node/scripts/build-bindgen.sh` and `node/scripts/generate.sh`: pinned UniFFI
  TypeScript generator build and native binding generation. The generated
  colocated library must be named `libserver.so`.
- `node/scripts/build.mjs`, `node/scripts/prepack.mjs`, and
  `node/scripts/packaging.mjs`: ESM/declaration build, package allowlist, ELF
  architecture and loader-metadata gates, and RPATH removal.
- `node/scripts/test-package.mjs`: installs the exact supplied native tarball in
  an isolated directory outside Nix, checks ELF/license/dependencies, runs a
  strict TypeScript consumer, clears loader overrides, and executes the engine.
- `node/tests/packaging.test.mjs`: release-tag, version staging, package payload,
  ELF, dependency, and ESM conversion unit tests.
- `clients/typescript/scripts-build.mjs`, `scripts-prepack.mjs`, and
  `scripts-test-package.mjs`: compiled HTTP ESM/declarations and isolated exact
  tarball import/typecheck test.
- `flake.nix`: exposes `packages.npm-client` as a real Nix tarball derivation.
- `nix/npm-native.nix`: builds and packs a staged native source tree as a Nix
  derivation after the engine and generated bindings exist.
- `docs/npm-publishing.md`: operator setup, bootstrap, OIDC, release, verification,
  and rollback runbook.

## Build model and portability truth

The HTTP tarball is directly produced by `nix build .#npm-client`.

The native path is intentionally hybrid. Cargo first builds the real
`mcp-v8-uniffi-python` shared engine from source, and the pinned generator emits
Node bindings around it. A content-addressed staged source tree containing that
engine and generated code is then compiled, stripped of RPATH, and packed by
`nix/npm-native.nix`. Therefore the npm tarball is Nix-produced, but the entire
source-V8 build is not currently an independently substitutable Nix output.
Never describe the GitHub Actions source-V8 cache as a Nix binary cache.

The ordinary pinned rusty_v8 release archive is not usable here: run 34153857783
failed while linking a shared object because the archive contains
`R_X86_64_TPOFF32` relocations. Keep the source build with:

```sh
unset RUSTY_V8_ARCHIVE
export V8_FROM_SOURCE=1
export GN_ARGS="v8_monolithic=true v8_monolithic_for_shared_library=true"
```

GitHub Actions uses a compatible cache identity beginning with
`python-uniffi-v8-145-e6a88b35-shared-v3-Linux-`, keyed by the pinned V8/toolchain
and linker inputs rather than application or JavaScript edits. A local machine
will still perform a potentially long source build unless it already has a
compatible Cargo target tree; the Actions cache is not automatically available
as a local or Nix substitute.

Native portability must remain fail-closed. Do not mutate arbitrary binary bytes
to hide build paths. Remove RPATH through normal ELF tooling, inspect dynamic
metadata with `readelf`, resolve dependencies with `ldd` outside Nix, reject
unresolved/store/workspace loader references, clear `LD_LIBRARY_PATH` and
`LD_PRELOAD`, and execute the installed tarball.

## Requirements

For native validation, use Linux x64 with glibc. Install Nix with flakes enabled,
Git, and Node.js. The validation workflows use Node.js 22. Trusted publication
uses Node.js 24 and explicitly requires Node.js 22.14.0 or newer plus npm 11.5.1
or newer. The Nix development shell supplies the pinned Rust/Cargo and native
build dependencies. `readelf` and `ldd` must be available for the outside-Nix
consumer gate. A cold source-V8 build can take substantial time and disk space.

## Exact local build and test procedure

Run this only after the owner chooses `VERSION`. Use a clean disposable
worktree because version staging modifies `package.json` and `package-lock.json`.
These commands reproduce the current workflow without uploading anything.

```sh
set -euo pipefail
VERSION='OWNER_APPROVED_VERSION'
OUT="$PWD/local-npm-artifacts/$VERSION"
mkdir -p "$OUT/client" "$OUT/node"

# Validate the shared version and stage it into both package/lock manifests.
node node/scripts/release-tag.mjs "v$VERSION"
node node/scripts/release-tag.mjs --stage clients/typescript "$VERSION"
node node/scripts/release-tag.mjs --stage node "$VERSION"

# HTTP client: build one Nix tarball, then test that exact file outside Nix.
nix build .#npm-client
npm ci --include=dev --prefix clients/typescript
npm run typecheck --prefix clients/typescript
npm run build --prefix clients/typescript
timeout 180s node clients/typescript/scripts-test-package.mjs result/*.tgz
cp --dereference result/*.tgz "$OUT/client/"

# Native engine: force the PIC-compatible source-V8 configuration.
nix develop --command bash -c '
  unset RUSTY_V8_ARCHIVE
  export V8_FROM_SOURCE=1
  export GN_ARGS="v8_monolithic=true v8_monolithic_for_shared_library=true"
  export CARGO_TARGET_DIR="$PWD/target/python-uniffi"
  cargo build -p mcp-v8-uniffi-python --release \
    --config mcp-v8-uniffi-python/cargo-config.toml
'

nix develop --command bash node/scripts/build-bindgen.sh
npm ci --include=dev --prefix node
npm run test:unit --prefix node
nix develop --command bash node/scripts/generate.sh \
  target/python-uniffi/release/libmcp_v8_uniffi.so

stage="$(mktemp -d)"
tar --exclude=node/node_modules --exclude=node/dist -cf - LICENSE node \
  | tar -C "$stage" -xf -
NATIVE_NPM_SOURCE="$stage" nix build --impure --expr '
  let
    flake = builtins.getFlake (toString ./.);
    pkgs = flake.inputs.nixpkgs.legacyPackages.${builtins.currentSystem};
  in import ./nix/npm-native.nix {
    inherit pkgs;
    src = builtins.path {
      path = /. + builtins.getEnv "NATIVE_NPM_SOURCE";
      name = "mcp-js-node-source";
    };
  }
'

npm run typecheck --prefix node
timeout 90s npm test --prefix node
nix develop --command npm run build --prefix node
timeout 240s node node/scripts/test-package.mjs result/*.tgz
cp --dereference result/*.tgz "$OUT/node/"

(cd "$OUT" && sha256sum client/*.tgz node/*.tgz > SHA256SUMS)
```

Before any bootstrap decision, inspect each embedded manifest rather than relying
on its filename:

```sh
for tarball in "$OUT"/*/*.tgz; do
  echo "$tarball"
  tar -xOf "$tarball" package/package.json \
    | node -e 'let s=""; process.stdin.on("data", d => s += d).on("end", () => { const p=JSON.parse(s); console.log({name:p.name, version:p.version, license:p.license}); });'
done
cat "$OUT/SHA256SUMS"
```

Do not commit `local-npm-artifacts/`, generated libraries, bindings, `dist/`, or
tarballs. Do not run an upload action merely to transfer local files while the
stop boundary remains in force.

## Current evidence

- PR: https://github.com/r33drichards/mcp-js/pull/259 (draft).
- `34176267690`: success on `d015af3c`; native and HTTP Nix builds, both exact
  outside-Nix consumers, and artifact-upload steps passed. This upload completed
  before the owner stop was received:
  https://github.com/r33drichards/mcp-js/actions/runs/34176267690
- `34175323579`: success on `68527834`; native and HTTP Nix builds plus both
  installed consumers passed before upload steps were added:
  https://github.com/r33drichards/mcp-js/actions/runs/34175323579
- `34175296968`: a release-workflow dispatch from the PR branch skipped, proving
  the manual path fails closed away from protected `main`:
  https://github.com/r33drichards/mcp-js/actions/runs/34175296968
- `34153857783`: expected failure proving the upstream rusty_v8 archive cannot
  link this shared library:
  https://github.com/r33drichards/mcp-js/actions/runs/34153857783
- `d015af3c` is the last code-bearing head with full package evidence. The
  successor commit adding this handoff is documentation-only and intentionally
  does not start another upload-enabled run.
- Local packaging unit tests passed at `d015af3c`, including stable/prerelease
  routing, malformed tag rejection, version staging, payload, ELF, dependency,
  and ESM conversion cases.
- At the last read-only registry check, both package names returned HTTP 404; no
  npm publication, release tag, GitHub release, or merge had occurred.

## Remaining work and owner-only decisions

1. Review this large draft PR and decide whether to split/squash its history.
   Do not rewrite the shared branch without explicit approval.
2. Ask the owner to select the actual bootstrap version. The prior `0.21.0`
   artifact validation is not version approval.
3. Decide whether to keep, remove, or condition the upload steps currently in
   `.github/workflows/node-uniffi-e2e.yml`. Do not trigger them while uploads are
   forbidden.
4. Re-run the full exact-head native and HTTP validation after any code change.
   If uploads are still forbidden, use the local procedure above or an expressly
   non-uploading workflow path.
5. Have an authorized owner bootstrap each absent npm package exactly once from
   approved local tarballs. Authentication must happen interactively on the
   owner's machine with normal npm login and 2FA. Never request, paste, log, or
   transmit credentials or setup links. Do not claim local provenance and do not
   use local `--provenance`.
6. After both package pages exist, configure npm trusted publishing separately
   for each package: owner `r33drichards`, repository `mcp-js`, workflow filename
   `npm-publish.yml`, environment `npm-publish`, direct publish action.
7. Create the protected GitHub `npm-publish` Environment with required reviewer
   approval and deployment tags restricted to `v*.*.*`. Protect creation,
   update, and deletion of those release tags. Add no npm token or
   `NODE_AUTH_TOKEN` fallback.
8. Only after owner approval, environment/tag protection, successful bootstrap,
   and an exact final-head validation may the ordinary shared release tag be
   considered. The tag-triggered job publishes both exact validated tarballs
   with OIDC provenance: stable to `latest`, prerelease to `next`.
9. Verify published versions, integrity, installed imports/native execution,
   signatures, and npm provenance. Roll forward with a new patch on defects;
   published versions are immutable.

Stop and hand control back to the owner before any external side effect. The
next agent's immediate task is review and local validation, not publication.
