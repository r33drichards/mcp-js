# npm publishing runbook

This repository has two public npm package candidates:

| Package | Source directory | License | Supported release target |
| --- | --- | --- | --- |
| `@wholelottahoopla/mcp-js-node` | `node/` | AGPL-3.0-only; verbatim root text in `node/LICENSE` | Linux x64 with glibc only |
| `@wholelottahoopla/mcp-js-client` | `clients/typescript/` | MIT; scoped text in `clients/typescript/LICENSE` | Node.js 18+ HTTP client |

Do not publish either package until every gate in this document is satisfied.
The repository's [`npm-publish.yml`](../.github/workflows/npm-publish.yml) is
intentionally release/manual-only; pull requests and pushes cannot publish.
It has no npm token fallback. The only job granted `id-token: write` is the
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

1. **Native Linux portability must pass CI.** The native package's `prepack`
   rejects missing/non-x64 ELF files, RPATH/RUNPATH, absolute `DT_NEEDED`,
   unresolved libraries, and Nix-store dependency resolutions. The full native
   build and installed-tarball execution must pass on GitHub's Ubuntu runner;
   a local Nix build is not sufficient proof.
2. **First-publication bootstrap is manual.** npm trusted-publisher settings
   are configured in an existing package's npm settings. Because both scoped
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
   approval from the owner/release managers, restrict deployment branches/tags
   to protected `main` and release tags, and do not add npm credentials to that
   environment. Configure branch and tag protection so only authorized people
   can create a release from `main`.
4. On npm, after OIDC publishing has succeeded, set Publishing access to
   **Require two-factor authentication and disallow tokens**, then revoke any
   legacy automation tokens. This blocks long-lived token publishing while
   leaving OIDC trusted publishing functional.

npm's trusted-publisher configuration is case-sensitive and cannot be edited
in place. If its repository, workflow filename, or environment is wrong,
delete it and create a new configuration. npm supports cloud-hosted runners;
the workflow uses `ubuntu-24.04` and Node 24, and checks npm CLI >=9.5.0.
Public OIDC publication from this public repository produces provenance
attestations automatically; the workflow also explicitly requests
`--provenance`.

## First publication bootstrap

Only do this after resolving all blockers, reviewing the generated tarball, and
confirming with the owner that this is the intended public release. The human
owner must use npm's normal interactive authentication/2FA locally; this is
not a request to store a credential in this repository or in GitHub Actions.

1. Choose the shared semver version, for example `0.1.0`. Both package manifests
   must use that exact version and the release tag must be `v0.1.0`.
2. Run the package validation commands in the next section. For the native
   package, run the full build outside `nix develop` before packing so the
   dynamic dependency gate evaluates the consumer environment.
3. Inspect the `.tgz` contents and publish each approved package manually with
   `npm publish --access public --provenance` from its package directory. The
   first public scoped publish requires `--access public`.
4. Verify the release using the commands below. Then complete the trusted
   publisher and GitHub Environment setup above before a second release.

## Release checklist after OIDC setup

1. Update both package versions to the selected shared semver value and commit
   the changes to protected `main`. Create a protected annotated tag named
   `vVERSION` from that exact `main` commit, then publish the GitHub release.
   This triggers `npm-publish.yml`; it requires the package versions to exactly
   match the tag and requires that commit to be an ancestor of `main`.
2. Alternatively use **Actions -> Publish npm Packages -> Run workflow** on
   `main`, enter the exact shared version and explicitly confirm. The workflow releases
   both packages together; use the protected tag/release flow for every release.
3. Approve the `npm-publish` Environment deployment. The workflow builds from
   locked dependencies, audits production dependencies, executes the relevant
   tarball tests, creates a tarball, verifies its name/version and required
   payload before publication, then runs `npm publish --access public
   --provenance` without an npm token.
4. Verify each result:

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

   Run the native install/import check on supported Linux x64 glibc only. Check
   npm's package page for provenance after publication as well.

## Rollback and incident response

Published versions are immutable and consumers may cache them. Do not rely on
unpublishing as a rollback plan. Stop/disable the GitHub Environment approval,
deprecate the affected version on npm with a clear upgrade target, and publish
a corrected patch version after the same validation and approval process.
For a credential or trusted-publisher incident, immediately remove the affected
trusted publisher in npm, revoke any legacy tokens, audit recent releases and
provenance, then restore a corrected publisher configuration only after review.
