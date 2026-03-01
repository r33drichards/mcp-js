{
  description = "MCP JS – JavaScript execution over the Model Context Protocol";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay, ... }:
    let
      # NixOS module – importable by any NixOS configuration
      nixosModules.mcp-js = import ./nix/module.nix;
    in
    (flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ rust-overlay.overlays.default ];
        };

        # Single Rust toolchain used for both building and development.
        # Using stable latest ensures all deps (including SWC) compile,
        # while rust-overlay keeps it consistent across nix build & nix develop.
        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [ "rust-src" "rustfmt" "clippy" ];
        };

        # Nightly toolchain for cargo-fuzz (requires -Z flags)
        rustNightly = pkgs.rust-bin.nightly.latest.default.override {
          extensions = [ "rust-src" "rustfmt" "clippy" ];
        };

        rustPlatform = pkgs.makeRustPlatform {
          cargo = rustToolchain;
          rustc = rustToolchain;
        };

        # Patched replace-workspace-values that handles the 'version' key
        # in workspace-inherited dependencies.  Upstream nixpkgs script
        # does not handle this, causing builds to fail for crates (like
        # rmcp) that specify  version = "X"  alongside  workspace = true.
        patchedReplaceWorkspaceValues = pkgs.writers.writePython3Bin
          "replace-workspace-values"
          {
            libraries = with pkgs.python3Packages; [ tomli tomli-w ];
            flakeIgnore = [ "E501" "W503" ];
          }
          (builtins.readFile ./nix/replace-workspace-values.py);

        # Pre-fetched rusty_v8 static library.
        # The v8 crate's build.rs tries to download this at build time,
        # which fails inside the Nix sandbox and on network-restricted
        # CI runners.  Pre-fetching and setting RUSTY_V8_ARCHIVE avoids
        # any network access during build.
        rustyV8Version = "145.0.0";
        rustyV8Target = {
          "x86_64-linux"   = "x86_64-unknown-linux-gnu";
          "aarch64-linux"  = "aarch64-unknown-linux-gnu";
          "x86_64-darwin"  = "x86_64-apple-darwin";
          "aarch64-darwin" = "aarch64-apple-darwin";
        }.${system};
        rustyV8Archive = pkgs.fetchurl {
          url = "https://github.com/denoland/rusty_v8/releases/download/v${rustyV8Version}/librusty_v8_release_${rustyV8Target}.a.gz";
          hash = {
            "x86_64-linux"   = "sha256-chV1PAx40UH3Ute5k3lLrgfhih39Rm3KqE+mTna6ysE=";
            "aarch64-linux"  = "sha256-4IivYskhUSsMLZY97+g23UtUYh4p5jk7CzhMbMyqXyY=";
            "x86_64-darwin"  = "sha256-1jUuC+z7saQfPYILNyRJanD4+zOOhXU2ac/LFoytwho=";
            "aarch64-darwin" = "sha256-yHa1eydVCrfYGgrZANbzgmmf25p7ui1VMas2A7BhG6k=";
          }.${system};
        };
      in {
        devShells.default = import ./shell.nix { inherit pkgs rustToolchain rustyV8Archive; };
        devShells.fuzz = import ./shell.nix { inherit pkgs rustyV8Archive; rustToolchain = rustNightly; };

        # SQLite compiled to WASM via Emscripten — used by the sqlite-wasm example.
        packages.sqlite-wasm = import ./nix/sqlite-wasm.nix { inherit pkgs; };

        packages.default = rustPlatform.buildRustPackage {
          pname = "mcp-js-server";
          version = "0.1.0";
          src = ./server;

          # Use cargoDeps with a patched vendor step instead of cargoHash
          # so we can inject our fixed replace-workspace-values script.
          cargoDeps = (rustPlatform.fetchCargoVendor {
            src = ./server;
            hash = "sha256-yQgwtqWhtTqcJCmhq8poouLxYn2ygpzCCMzv9VktLK0=";
          }).overrideAttrs (old: {
            nativeBuildInputs = map (dep:
              if (dep.name or "") == "replace-workspace-values"
              then patchedReplaceWorkspaceValues
              else dep
            ) (old.nativeBuildInputs or []);
          });

          nativeBuildInputs = with pkgs; [
            clang
            git
            llvmPackages.bintools
            pkg-config
          ];

          buildInputs = with pkgs; [
            openssl
          ];

          LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";

          # Point v8 build.rs to the pre-fetched static library so it
          # doesn't try to download during the sandboxed build.
          RUSTY_V8_ARCHIVE = "${rustyV8Archive}";

          # Integration/e2e tests start servers and make HTTP requests,
          # which does not work inside the Nix build sandbox.  The NixOS
          # VM test (checks.x86_64-linux.cluster-test) covers integration
          # testing instead.
          doCheck = false;

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
          load-balancing-test = pkgs.nixosTest (import ./tests/nixos/load-balancing.nix {
            inherit pkgs;
            mcp-js = self.packages.x86_64-linux.default;
          });
          fetch-opa-test = pkgs.nixosTest (import ./tests/nixos/fetch-opa.nix {
            inherit pkgs;
            mcp-js = self.packages.x86_64-linux.default;
          });
          fs-opa-test = pkgs.nixosTest (import ./tests/nixos/fs-opa.nix {
            inherit pkgs;
            mcp-js = self.packages.x86_64-linux.default;
          });
          module-policy-test = pkgs.nixosTest (import ./tests/nixos/module-policy.nix {
            inherit pkgs;
            mcp-js = self.packages.x86_64-linux.default;
          });
        };
    };
}
