# Install the Server

## Install from the release script

```bash
curl -fsSL https://raw.githubusercontent.com/r33drichards/mcp-js/main/install.sh | sudo bash
```

This installs `mcp-v8` into `/usr/local/bin/`.

## Build from source

```bash
cargo build --release -p server
./target/release/server --help
```

Building from source is the better option when you are testing changes, using a
development branch, or working in an environment where you already have Rust
tooling available.
