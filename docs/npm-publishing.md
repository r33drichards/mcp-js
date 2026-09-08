# npm publishing runbook

This repository has two public npm package candidates:

| Package | Source directory | License | Supported release target |
| --- | --- | --- | --- |
| `@wholelottahoopla/mcp-js-node` | `node/` | AGPL-3.0-only; verbatim root text in `node/LICENSE` | Linux x64 with glibc only |
| `@wholelottahoopla/mcp-js-client` | `clients/typescript/` | MIT; scoped text in `clients/typescript/LICENSE` | Node.js 18+ HTTP client |

Do not publish either package until every gate in this document is satisfied.
The repository's [`npm-publish.yml`](../.github/workflows/npm-publish.yml) is
intentionally tag/manual-only. Manual runs only validate artifacts; only
protected `node-v*` and `client-v*` tag pushes can publish. It has no npm token
fallback. The only job granted `id-token: write` is the
GitHub Environment-protected `publish` job.

## Licensing split

The owner approved a deliberate package-level split: the HTTP client
`@wholelottahoopla/mcp-js-client` is MIT-licensed and ships its own MIT
`LICENSE`; the native `@wholelottahoopla/mcp-js-node` package remains
AGPL-3.0-only and ships the verbatim root AGPL text. The HTTP copyright
attribution is grounded in the repository's pre-AGPL license commit
`22a0bddb` and the TypeScript-client creation commits `8ba7855a` and
`1f92b993`; no new rights-holder identity was invented.

## Current release blockers

1. **Revalidate the exact release head.** The native Linux portability gate has
   passed on PR head `5532fcb3` in CI run `34160872507`; any later release
   change must pass the same exact Nix-tarball consumer gate. The native package's `prepack`
   rejects missing/non-x64 ELF files, RPATH/RUNPATH, absolute `DT_NEEDED`,
   unresolved libraries, Nix-store dependency resolutions, and Nix-store or CI
   workspace references in runtime loader metadata. The full native build and installed-tarball execution
   must pass on GitHub's Ubuntu runner; a local Nix build is not sufficient proof.
2. **First-publication bootstrap is manual.** npm trusted-publisher settings
   can only be configured in an existing package's npm settings. Because both scoped
   package names are currently absent from npm, an owner must perform the first
   approved public release manually before OIDC can be attached. Do not add an
   npm token to GitHub Actions to bypass this limitation.

## One-time owner setup after the first release

Perform these steps separately for **each** package on npmjs.com:

1. Confirm the scope `@wholelottahoopla` is owned by the publishing npm account
   and that the package's `repository.url` exactly remains
   `https://github.com/r33drichards/mcp-js`.
2. Open **Packages -> PACKAGE -> Settings -> Trusted publishing** and add a
   GitHub Actions trusted publisher with these exact values:
   - Organization or user: `r33drichards`
   - Repository: `mcp-js`
   - Workflow filename: `npm-publish.yml` (filename only)
   - Environment name: `npm-publish`
   - Allowed action: permit direct `npm publish` (the release workflow uses
     direct publication, not staged publishing).
3. In the GitHub repository, create the `npm-publish` Environment. Require
   approval from the owner/release managers, allow deployment tags matching
   only `node-v*` and `client-v*`, and do not add npm credentials to that
   environment. Add repository tag rules for those patterns that restrict tag
   creation, update, and deletion to release managers. Tags must target a commit
   already contained in protected `main`; the workflow verifies this again.
4. On npm, after OIDC publishing has succeeded, set Publishing access to
   **Require two-factor authentication and disallow tokens**, then revoke any
   legacy automation tokens. This blocks long-lived token publishing while
   leaving OIDC trusted publishing functional.

npm's trusted-publisher configuration is case-sensitive and cannot be edited
in place. If its repository, workflow filename, or environment is wrong,
delete it and create a new configuration. npm supports cloud-hosted runners;
the workflow uses `ubuntu-24.04` and Node 24, and explicitly requires Node.js >=22.14.0 and npm >=11.5.1 for trusted publishing (rather than the older npm 9.5.0 provenance-only baseline).
Public OIDC publication from this public repository produces provenance
attestations automatically; the workflow also explicitly requests
`--provenance`.

## First publication bootstrap

Both package names currently return HTTP 404 from the npm registry, so npm has
no package settings page on which to add a trusted publisher. Bootstrap each
package once with an owner's normal interactive npm login and 2FA. Do not add
that credential to GitHub, and do not use `--provenance` locally: npm provenance
requires a supported cloud CI runner.

The two packages version independently. Set and commit only the selected
package manifest version on protected `main`; selecting `both` does not require
the two version inputs to match. Then run **Publish npm Packages** manually on
`main` with:

- Mode: `validate`
- Package: `node`, `client`, or `both`
- Node version: the exact `node/package.json` version if node is selected
- Client version: the exact `clients/typescript/package.json` version if client is selected
- Confirm publish: unchecked

This mode cannot reach the OIDC publish job. Wait for the selected build job,
download `npm-tarball-node` and/or `npm-tarball-client` from the run's
**Artifacts** section, and inspect the archive. These are the exact Nix-produced
tarballs that passed their outside-Nix installed-consumer tests. Publish each
approved bootstrap artifact interactively:

```sh
npm login
npm publish path/to/wholelottahoopla-mcp-js-node-VERSION.tgz --access public
npm publish path/to/wholelottahoopla-mcp-js-client-VERSION.tgz --access public
```

Run only the command for the selected package. Complete the trusted-publisher
setup above immediately after each bootstrap publication, then run the
verification commands. The first publication has no provenance attestation;
subsequent OIDC publications do.

## Reproducible builds and native cache

Build the HTTP client tarball as a real Nix derivation with
`nix build .#npm-client`; the output directory contains the scoped `.tgz`.
The HTTP package E2E and release workflows install this exact derivation
output in an independent outside-Nix consumer test. Native CI generates the
bindings from the real shared engine, stages that content-addressed source tree, and evaluates
`nix/npm-native.nix`; this makes compilation, RPATH removal,
and packing a Nix derivation too. The independent test then installs the exact
Nix-produced tarball outside Nix, checks its installed ELF with `readelf` and `ldd`,
and executes the engine. Packing inside Nix disables lifecycle scripts because
prepack's outside-Nix loader checks cannot run meaningfully in that sandbox;
the mandatory installed-tarball gate performs ELF, license, dependency,
TypeScript, and engine execution checks before release artifacts are uploaded.

This native derivation consumes an externally generated engine and bindings.
It is not yet a complete native flake output or an independently substitutable
source-V8 derivation. The Cargo cache below is not a Nix binary cache.

The native package cannot use rusty_v8's ordinary release archive: CI run
`34153857783` proved that it contains `R_X86_64_TPOFF32` relocations that the
linker rejects in a shared object. Native CI therefore uses the pinned
rusty_v8 revision with `v8_monolithic_for_shared_library=true`. Its persisted
cache key contains the V8 version/revision, GN mode, runner target, pinned Nix
toolchain, and cargo linker configuration, but deliberately excludes server,
Node, and application source. The cache is saved immediately after the costly
build, before package checks, so application or packaging changes reuse V8 and
rebuild only affected Rust crates. The release workflow consumes the same
cache. This cache is build acceleration only: every release still links the
engine, removes build-host RPATH, validates dynamic dependencies outside Nix,
and executes an installed tarball.

## Release checklist after OIDC setup

The packages release independently from protected tags:

1. Set and commit the selected package version on protected `main`.
2. Optionally run **Publish npm Packages** manually on `main` to build and
   inspect artifacts without publishing. Manual dispatch has no publish mode.
3. Create and push one annotated tag at that exact `main` commit:

   ```sh
   git tag -a node-vVERSION -m "Release @wholelottahoopla/mcp-js-node VERSION"
   git push origin node-vVERSION
   # Or, independently:
   git tag -a client-vVERSION -m "Release @wholelottahoopla/mcp-js-client VERSION"
   git push origin client-vVERSION
   ```

   The tag prefix selects exactly one package. Its semver must exactly match
   that package manifest, and its peeled commit must equal the workflow SHA and
   be an ancestor of `origin/main`. Stable versions publish under npm `latest`;
   prerelease versions such as `client-v1.2.0-rc.1` publish under `next`, so a
   prerelease cannot replace `latest` accidentally.
4. Approve the protected `npm-publish` Environment deployment. The workflow
   publishes only the exact Nix tarball that passed its outside-Nix consumer,
   identity, and payload gates, using OIDC with provenance.
5. Verify the selected result:

   ```sh
   npm view @wholelottahoopla/mcp-js-node@VERSION version dist.integrity
   npm view @wholelottahoopla/mcp-js-client@VERSION version dist.integrity
   mkdir /tmp/mcp-js-npm-smoke && cd /tmp/mcp-js-npm-smoke
   npm init -y
   npm install @wholelottahoopla/mcp-js-client@VERSION
   node --input-type=module -e 'import("@wholelottahoopla/mcp-js-client").then(() => console.log("client ok"))'
   npm install @wholelottahoopla/mcp-js-node@VERSION
   node --input-type=module -e 'import("@wholelottahoopla/mcp-js-node").then(() => console.log("native import ok"))'
   npm audit signatures
   ```

   Run only checks for packages released in this invocation. Run native checks
   on supported Linux x64 glibc and confirm npm's package page shows provenance.

## Rollback and incident response

Published versions are immutable and consumers may cache them. Do not rely on
unpublishing as a rollback plan. Stop/disable the GitHub Environment approval,
deprecate the affected version on npm with a clear upgrade target, and publish
a corrected patch version after the same validation and approval process.
For a credential or trusted-publisher incident, immediately remove the affected
trusted publisher in npm, revoke any legacy tokens, audit recent releases and
provenance, then restore a corrected publisher configuration only after review.
