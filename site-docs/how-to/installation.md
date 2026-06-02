# How to Install mcp-v8

Install the mcp-v8 server and optional CLI client.

## Install the server via script

```bash
curl -fsSL https://raw.githubusercontent.com/r33drichards/mcp-js/main/install.sh | sudo bash
```

This detects your OS and architecture, downloads the latest release binary, and installs it to `/usr/local/bin/mcp-v8`.

To pin a specific version:

```bash
MCP_V8_VERSION=v0.1.0 curl -fsSL https://raw.githubusercontent.com/r33drichards/mcp-js/main/install.sh | sudo bash
```

## Install the CLI client

```bash
curl -fsSL https://raw.githubusercontent.com/r33drichards/mcp-js/main/install-cli.sh | sudo bash
```

Installs `mcp-v8-cli` to `/usr/local/bin/`.

## Install with Cargo

```bash
cd server
cargo install --path .
```

For the CLI:

```bash
cd mcp-v8-client
cargo install --path .
```

## Build from source

```bash
git clone https://github.com/r33drichards/mcp-js.git
cd mcp-js/server
cargo build --release
```

The binary is at `target/release/mcp-v8`.

## Install with Nix

```bash
nix build
```

For NixOS, a module is available at `nix/module.nix`:

```nix
{
  services.mcp-js = {
    enable = true;
    package = pkgs.mcp-v8;
    nodeId = "node1";
    httpPort = 3000;
    dataDir = "/var/lib/mcp-js";
  };
}
```

## Verify the installation

```bash
mcp-v8 --version
```
