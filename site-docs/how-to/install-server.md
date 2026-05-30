# Install the Server

## Install from the release script

```bash
curl -fsSL https://raw.githubusercontent.com/r33drichards/mcp-js/main/install.sh | sudo bash
```

This installs `mcp-v8` into `/usr/local/bin/`.

The rest of the how-to pages assume that `mcp-v8` is available on your `PATH`
after this install.

## Download a release directly

If you want a specific version or prefer to download binaries yourself, use the
GitHub Releases page:

```text
https://github.com/r33drichards/mcp-js/releases
```

That page includes pre-built binaries and older release versions.

## Build from source

```bash
git clone https://github.com/r33drichards/mcp-js.git
cd mcp-js
cargo build --release -p server
./target/release/server --help
```

Building from source is the better option when you are testing changes, using a
development branch, or working in an environment where you already have Rust
tooling available. In that case, use `./target/release/server` in the examples
below anywhere this documentation says `mcp-v8`.

## Install or run with Nix

If you use Nix, the repo already exposes a flake package and development shell.

Build the packaged binary:

```bash
git clone https://github.com/r33drichards/mcp-js.git
cd mcp-js
nix build .#default
./result/bin/server --help
```

Run it directly without keeping the build output:

```bash
nix run .#default -- --help
```

Open a development shell when you want the full toolchain for local builds:

```bash
nix develop
```
