{
  description = "MCP JS – JavaScript execution over the Model Context Protocol";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  inputs.flake-utils.url = "github:numtide/flake-utils";

  outputs = { self, nixpkgs, flake-utils, ... }:
    let
      # NixOS module – importable by any NixOS configuration
      nixosModules.mcp-js = import ./nix/module.nix;
    in
    (flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };
      in {
        devShells.default = import ./shell.nix { inherit pkgs; };

        # Package – builds the server binary using rustPlatform.
        # NOTE: v8 (rusty_v8) requires a pre-built static library.  Set
        # RUSTY_V8_ARCHIVE to the path of the .a file if the automatic
        # download does not work inside the Nix sandbox.
        packages.default = pkgs.rustPlatform.buildRustPackage {
          pname = "mcp-js-server";
          version = "0.1.0";
          src = ./server;
          cargoLock = {
            lockFile = ./server/Cargo.lock;
            outputHashes = {
              "rmcp-0.1.5" = "sha256-3IPIlk1zIIemtJ4YeWgV4Qe3NyyR0I/nvDeqDebxyl4=";
            };
          };

          nativeBuildInputs = with pkgs; [
            clang
            llvmPackages.bintools
            pkg-config
          ];

          buildInputs = with pkgs; [
            openssl
          ];

          LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";

          # Allow network access for rusty_v8 binary download during build.
          # In a strict sandbox you must prefetch the archive and point
          # RUSTY_V8_ARCHIVE to it instead.
          __noChroot = true;

          meta.mainProgram = "server";
        };
      }
    )) // {
      inherit nixosModules;

      # NixOS integration test – run with:
      #   nix build .#checks.x86_64-linux.cluster-test
      checks.x86_64-linux =
        let
          pkgs = import nixpkgs { system = "x86_64-linux"; };
        in {
          cluster-test = pkgs.nixosTest (import ./tests/nixos/cluster.nix {
            inherit pkgs;
            mcp-js = self.packages.x86_64-linux.default;
          });
        };
    };
}
