# Install: Build from Source

Build from source when you are testing changes, using a development branch, or
working in an environment where you already have Rust tooling available.

```bash
git clone https://github.com/r33drichards/mcp-js.git
cd mcp-js
cargo build --release -p server
./target/release/server --help
```

In that case, use `./target/release/server` in examples anywhere the docs say
`mcp-v8`.
