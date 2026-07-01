# Vendored test fixtures

These files are committed so the NixOS integration tests run **fully offline**
(NixOS test VMs have no internet, so `import npm:…` cannot fetch at runtime).

## `isomorphic-git-bundle.mjs`

A self-contained ESM bundle of [`isomorphic-git`](https://isomorphic-git.org/)
(`1.27.1`) plus its `http/web` client and all dependencies (including a `Buffer`
polyfill), exporting `{ git, http }`. It depends only on the global
`TextEncoder`/`TextDecoder` (provided by the runtime) and the sandbox `fetch`.

Used by `tests/nixos/isomorphic-git.nix` to prove the sandbox `fs` object is a
working Node-style filesystem: it drives `git.init` → write → `git.add` →
`git.commit` → `git.push` against a real mcp-v8 server.

### Regenerating

```sh
npm i isomorphic-git@1.27.1 esbuild buffer
npx esbuild entry.mjs --bundle --format=esm --platform=browser \
  --target=es2022 --inject:bufinject.js --outfile=isomorphic-git-bundle.mjs
```

`entry.mjs` and `bufinject.js` (the esbuild entry point and the Buffer/process
inject shim) are kept alongside the bundle to document exactly how it was built.
