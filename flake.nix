{
  description = "MCP V8 JavaScript execution server";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    crane = {
      url = "github:ipetkov/crane";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, flake-utils, crane, rust-overlay, ... }:
    (flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ (import rust-overlay) ];
        };

        # Read rust toolchain version
        rustToolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain;

        craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchain;

        # Source filtering for crane
        src = craneLib.cleanCargoSource ./.;

        # Common build arguments
        commonArgs = {
          inherit src;
          strictDeps = true;

          # Build from server subdirectory
          pname = "mcp-v8";
          version = "0.1.0";

          # Point to the server subdirectory
          cargoExtraArgs = "--manifest-path=server/Cargo.toml";

          # Point to Cargo.lock in server directory
          cargoLock = ./server/Cargo.lock;

          nativeBuildInputs = with pkgs; [
            pkg-config
            rustPlatform.bindgenHook
          ];

          buildInputs = with pkgs; [
            openssl
            python3
          ];

          # V8 requires specific environment variables
          LIBCLANG_PATH = "${pkgs.llvmPackages_latest.libclang.lib}/lib";
        };

        # Build just the dependencies (for caching)
        cargoArtifacts = craneLib.buildDepsOnly commonArgs;

        # Build the actual binary
        mcp-v8 = craneLib.buildPackage (commonArgs // {
          inherit cargoArtifacts;

          # Rename the binary after build
          postInstall = ''
            mv $out/bin/server $out/bin/mcp-v8
          '';
        });

      in {
        # Development shell
        devShells.default = import ./shell.nix { inherit pkgs; };

        # Package output
        packages = {
          default = mcp-v8;
          inherit mcp-v8;
          docker = pkgs.callPackage ./docker.nix { mcp-v8 = mcp-v8; };
        };

        # Apps for running the server
        apps.default = {
          type = "app";
          program = "${mcp-v8}/bin/mcp-v8";
        };

        # Checks for CI
        checks = {
          inherit mcp-v8;
        };
      }));
}
