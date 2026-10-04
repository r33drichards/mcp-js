# pi harness against the native engine

This directory runs the [pi](https://github.com/r33drichards/pi) agent harness's
`McpJsExecutionEnv` and its `read`, `write`, `edit`, and `run_js` tools against
the real mcp-js shared library through the generated Node bindings. It is the
cross-repository proof that the typed native filesystem API and the pi adapter
agree; the adapter's own unit tests use an in-memory engine.

`.github/workflows/pi-harness-e2e.yml` builds the library, generates
`node/generated`, checks pi out into `pi-checkout/` at the repository root,
builds it, links `@earendil-works/pi-agent-core` from that checkout, and runs
`tests/harness.test.ts`. Locally, after following `node/README.md`:

```sh
git clone https://github.com/r33drichards/pi pi-checkout
npm ci --ignore-scripts --prefix pi-checkout
npm run build --prefix pi-checkout
npm ci --prefix node/pi-harness
npm run link-pi --prefix node/pi-harness   # or PI_CHECKOUT=/path/to/pi
npm test --prefix node/pi-harness
```

The link is a symlink rather than an npm `file:` dependency so that
pi-agent-core's own workspace dependencies resolve from the checkout, not the
registry. `pi-checkout/` and the link are ignored by git. The test is run with `tsx` and is not
typechecked: the generated binding types and pi's published types are the
contracts under test, and the assertions are runtime behavior.
