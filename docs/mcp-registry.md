# Publishing to the MCP Registry

The registry lists this Rust server as `io.github.r33drichards/mcp-js`, using
the existing public Docker Hub image. No npm package is needed.

`server.json` describes the image and its stdio invocation. The runtime-stage
Dockerfile label proves image ownership to the registry. Keep the label and
manifest name identical.

## Release through GitHub Actions

1. Merge the registry changes into `r33drichards/mcp-js`.
2. Ensure the repository's `DOCKERHUB_TOKEN` secret can push to
   `wholelottahoopla/mcp-js` and that the image is public.
3. Create and push a new, unused version tag on the merged commit, for example
   `git tag v0.20.1 && git push origin v0.20.1` (check availability first).
4. Watch **Release Rust Server**, then **Build and Push Docker Image**, which
   starts when the release workflow completes and packages that release's
   server binary. After the image succeeds, its `publish-mcp` job sets the
   manifest version and image tag from the release tag, authenticates using
   GitHub OIDC, and publishes the metadata.

No MCP Registry secret is required. The job needs `id-token: write`, and must
run in the repository owner's namespace. Existing release images do not gain
the new verification label retroactively; publish a new image from these changes.
The checked-in manifest uses `0.20.0` as a baseline, not a claim that that image
already contains the label. CI replaces it with the actual release version.

## Manual publication

After publishing a labeled image, update both `version` and the image tag in
`server.json` to that release. Install the official `mcp-publisher` CLI using
the [registry quickstart](https://modelcontextprotocol.io/registry/quickstart),
then run from the repository root:

```bash
mcp-publisher login github
mcp-publisher publish
curl --fail 'https://registry.modelcontextprotocol.io/v0.1/servers?search=io.github.r33drichards/mcp-js'
```

Authenticate as `r33drichards`. Do not commit registry authentication files.
If publication fails after the image push, rerun the failed registry job;
there is no need to rebuild the image.

## Test the advertised invocation

Substitute the newly published image version:

```bash
docker run --rm -i --env PORT= docker.io/wholelottahoopla/mcp-js:0.20.1 --stateless
```

The empty `PORT` overrides the image's HTTP default and selects stdio. Do not
allocate a TTY (`-t`), which can interfere with the MCP protocol. The registry
entry uses stateless mode to avoid requiring a persistent volume; policy-gated
host capabilities remain disabled by default. A Docker-capable client is
required. This listing does not advertise a shared hosted HTTP endpoint.
