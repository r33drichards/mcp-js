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

          # Use cargoDeps with a patched vendor step instead of cargoHash
          # so we can inject our fixed replace-workspace-values script.
          cargoDeps = (pkgs.rustPlatform.fetchCargoVendor {
            src = ./server;
            hash = "sha256-T/lxK1WeiumAZy8GzKeh9DiLqom2RMH2vvvNA0W+jw4=";
          }).overrideAttrs (old: {
            nativeBuildInputs = map (dep:
              if (dep.name or "") == "replace-workspace-values"
              then patchedReplaceWorkspaceValues
              else dep
            ) (old.nativeBuildInputs or []);
          });

          nativeBuildInputs = with pkgs; [
            clang
            llvmPackages.bintools
            pkg-config
            curl
            cacert
          ];

          buildInputs = with pkgs; [
            openssl
          ];

          LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";

          # Integration/e2e tests start servers and make HTTP requests,
          # which does not work inside the Nix build sandbox.  The NixOS
          # VM test (checks.x86_64-linux.cluster-test) covers integration
          # testing instead.
          doCheck = false;

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
          load-balancing-test = pkgs.nixosTest (import ./tests/nixos/load-balancing.nix {
            inherit pkgs;
            mcp-js = self.packages.x86_64-linux.default;
          });
        };
    };
}
