# Tutorial: Installing mcp-v8

In this tutorial you will install the mcp-v8 server and the mcp-v8-cli client using three different methods: the install script, Nix, and building from source. By the end you will have a working mcp-v8 binary ready to use.

## Prerequisites

- A Linux or macOS system
- `curl` installed (for the install script method)
- `nix` installed with flakes enabled (for the Nix method)
- Rust toolchain installed (for building from source)

## Method 1: Install Script (Recommended)

The fastest way to get started is the install script. It downloads a pre-built binary for your platform.

### Step 1: Run the server install script

```bash
curl -fsSL https://raw.githubusercontent.com/r33drichards/mcp-js/main/install.sh | sudo bash
```

This downloads the latest `mcp-v8` binary and places it on your PATH.

### Step 2: Verify the installation

```bash
mcp-v8 --help
```

You should see output listing all available flags and options:

```
Usage: mcp-v8 [OPTIONS]

Options:
  --http-port <PORT>        Start HTTP transport on this port
  --sse-port <PORT>         Start SSE transport on this port
  --directory-path <PATH>   Directory for heap storage
  ...
```

### Step 3: Install the CLI client

The CLI client is a separate binary for interacting with a running mcp-v8 server from the command line.

```bash
curl -fsSL https://raw.githubusercontent.com/r33drichards/mcp-js/main/install-cli.sh | sudo bash
```

### Step 4: Verify the CLI client

```bash
mcp-v8-cli --help
```

## Method 2: Nix

If you use Nix with flakes, mcp-v8 is available as a flake package.

### Step 1: Run directly with nix

You can try mcp-v8 without installing it:

```bash
nix run github:r33drichards/mcp-js
```

### Step 2: Install into your profile

To install it permanently:

```bash
nix profile install github:r33drichards/mcp-js
```

### Step 3: Use in a NixOS configuration

For NixOS systems, import the module from the flake:

```nix
# flake.nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    mcp-v8.url = "github:r33drichards/mcp-js";
  };

  outputs = { self, nixpkgs, mcp-v8 }: {
    nixosConfigurations.myhost = nixpkgs.lib.nixosSystem {
      modules = [
        mcp-v8.nixosModules.default
        {
          services.mcp-v8 = {
            enable = true;
            # Add any configuration options here
          };
        }
      ];
    };
  };
}
```

The NixOS module is defined in `nix/module.nix` in the repository.

### Step 4: Verify

```bash
mcp-v8 --help
```

## Method 3: Build from Source

Building from source gives you the latest development version.

### Step 1: Clone the repository

```bash
git clone https://github.com/r33drichards/mcp-js.git
cd mcp-js
```

### Step 2: Build with Cargo

```bash
cargo build --release
```

This compiles the project and places the binary at `target/release/mcp-v8`.

### Step 3: Copy the binary to your PATH

```bash
sudo cp target/release/mcp-v8 /usr/local/bin/
```

### Step 4: Build the CLI client

```bash
cargo build --release --bin mcp-v8-cli
sudo cp target/release/mcp-v8-cli /usr/local/bin/
```

### Step 5: Verify

```bash
mcp-v8 --help
mcp-v8-cli --help
```

## What you learned

- How to install mcp-v8 using the install script, Nix, or from source
- How to install the mcp-v8-cli client separately
- How to verify both binaries are working

Next, head to [Running Your First JS/TS Code](javascript-runtime.md) to start executing JavaScript.
