# Enable OPA Policies

Pass a JSON policy configuration through `--policies-json`:

```bash
./target/release/server \
  --stateless \
  --http-port 3000 \
  --policies-json '{"fetch":{"policies":[{"url":"file:///path/to/fetch.rego"}]}}'
```

`--policies-json` enables the policy chain used for fetch, module import
auditing, and filesystem access depending on the configuration you provide.
