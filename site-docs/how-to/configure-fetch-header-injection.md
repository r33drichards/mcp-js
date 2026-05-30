# Configure Fetch Header Injection

Add a static header rule on the command line:

```bash
./target/release/server \
  --stateless \
  --http-port 3000 \
  --policies-json '{"fetch":{"policies":[{"url":"file:///path/to/fetch.rego"}]}}' \
  --fetch-header 'host=api.github.com,header=Authorization,value=Bearer TOKEN,methods=GET'
```

Or load rules from a JSON file:

```bash
./target/release/server \
  --stateless \
  --http-port 3000 \
  --policies-json '{"fetch":{"policies":[{"url":"file:///path/to/fetch.rego"}]}}' \
  --fetch-header-config ./fetch-headers.json
```
