# Building with Nix and dockerTools

## Overview

This project now supports building Docker images using Nix's `dockerTools` instead of traditional Docker builds. This provides several advantages:

- **Reproducible builds**: Nix ensures bit-for-bit reproducibility
- **Better caching**: Nix's content-addressed storage provides excellent layer caching
- **No Docker daemon required**: Build images without running Docker
- **Smaller images**: More efficient layering reduces image size

## Current Status

⚠️ **Work in Progress**: There's currently an issue with the git dependency `rmcp` from the rust-sdk repository. The dependency uses Cargo workspace features that Nix's cargo vendoring doesn't fully support yet.

### The Problem

The `rmcp` crate is fetched from git and uses workspace-level dependency inheritance:
```toml
[dependencies]
rmcp-macros = { workspace = true, version = "0.1" }
```

Nix's `buildRustPackage` has trouble handling this pattern when vendoring dependencies.

## Workaround Options

### Option 1: Use crane (Recommended)

[crane](https://github.com/ipetkov/crane) is a better alternative to `buildRustPackage` that handles workspace dependencies more gracefully:

```nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    crane.url = "github:ipetkov/crane";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, crane, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };
        craneLib = crane.lib.${system};
      in {
        packages.default = craneLib.buildPackage {
          src = craneLib.cleanCargoSource (craneLib.path ./.);
          cargoExtraArgs = "--manifest-path=server/Cargo.toml";
          # ... other build inputs
        };
      });
}
```

### Option 2: Patch the dependency

We could fork the rust-sdk and remove the workspace inheritance, or patch it during the build.

### Option 3: Use naersk

Another alternative build system that may handle this better:

```nix
{
  inputs = {
    naersk.url = "github:nix-community/naersk";
    # ...
  };
}
```

### Option 4: Continue using traditional Docker + Nix for dependencies

Keep using the Dockerfile for builds but use Nix to provide the development environment.

## Files Created

- **`default.nix`**: Package definition for building the mcp-v8 binary
- **`docker.nix`**: dockerTools definition for creating the container image
- **`flake.nix`**: Updated with package outputs

## Usage (when working)

Once the workspace dependency issue is resolved:

### Build the package
```bash
nix build .#mcp-v8
```

### Build the Docker image
```bash
nix build .#docker
```

### Load the image into Docker
```bash
docker load < result
```

### Run directly with Nix
```bash
nix run .# -- --help
```

## Resources

- [Nix dockerTools documentation](https://ryantm.github.io/nixpkgs/builders/images/dockertools/)
- [crane documentation](https://github.com/ipetkov/crane)
- [buildRustPackage documentation](https://nixos.org/manual/nixpkgs/stable/#rust)
