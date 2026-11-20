# Install packages from GitHub Releases

Choose a version from [GitHub Releases](https://github.com/r33drichards/mcp-js/releases)
and inspect its assets. Replace every `VERSION` below with the same version
without the leading `v`, retaining a prerelease suffix such as `-rc.5`.
These commands install the exact release tarball, not an npm-registry version.
Only use a release that contains the requested asset; asset publication can
finish separately from creation of the release record.

## Native JavaScript engine SDK

```bash
npm install '@wholelottahoopla/mcp-js-node@https://github.com/r33drichards/mcp-js/releases/download/vVERSION/wholelottahoopla-mcp-js-node-VERSION.tar.gz'
```

The standard native asset targets Linux x64 with glibc 2.39 or newer and Node.js
22+. It embeds the engine in-process. For Alpine x64, choose a release with the
musl asset:

```bash
npm install '@wholelottahoopla/mcp-js-node@https://github.com/r33drichards/mcp-js/releases/download/vVERSION/wholelottahoopla-mcp-js-node-VERSION-linux-x64-musl.tar.gz'
```

The musl package is also tested with Bun on Alpine; replace `npm install` with
`bun add` to use it there. Server binary platform support is separate from this
native SDK matrix. See the [SDK guide](../how-to/native-javascript-sdk.md) for
execution, host tools, and disposal.

## TypeScript HTTP client

```bash
npm install '@wholelottahoopla/mcp-js-client@https://github.com/r33drichards/mcp-js/releases/download/vVERSION/wholelottahoopla-mcp-js-client-VERSION.tar.gz'
```

This is the HTTP client, not an embedded V8 engine. It needs a running server,
uses generated OpenAPI types, and supports Node.js 18+. Its MIT license is
separate from the native SDK's root AGPL-3.0 license.

## Server and CLI binaries

Use the release assets or [install scripts](overview.md#prebuilt-binary-install-scripts)
for the server and CLI. `MCP_V8_VERSION` selects the install-script release;
that environment variable does not change an npm tarball URL. Check the
[upgrade notes](../reference/release-upgrade.md) before changing runtime versions
or persistence formats.

## Source

- [Release package matrix and asset upload](https://github.com/r33drichards/mcp-js/blob/main/.github/workflows/npm-publish.yml)
- [Native SDK package checks](https://github.com/r33drichards/mcp-js/blob/main/node/scripts/packaging.mjs)
- [HTTP client](https://github.com/r33drichards/mcp-js/blob/main/clients/typescript/README.md)
