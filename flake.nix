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

        serverCargoDeps = (pkgs.callPackage ./nix/fetch-cargo-vendor.nix {
          cargo = rustToolchain;
        } {
          src = ./server;
          hash = "sha256-L1IGe3cHLFT35OQ/aJ4xq0RJqRZ4Yu6YbTSMf6ziMr0=";
        });

        docsPython = pkgs.python3.withPackages (
          ps: with ps; [
            mkdocs
            mkdocs-mermaid2-plugin
          ]
        );

        docsToolCargoFlags = [
          "--bin" "server"
          "--bin" "generate-cli-markdown"
          "--bin" "generate-mcp-tools-markdown"
        ];

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

        docsTools = rustPlatform.buildRustPackage {
          pname = "mcp-js-docs-tools";
          version = "0.1.0";
          src = ./server;

          cargoDeps = serverCargoDeps;

          cargoBuildFlags = docsToolCargoFlags;
          cargoInstallFlags = docsToolCargoFlags;

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
          RUSTY_V8_ARCHIVE = "${rustyV8Archive}";
          doCheck = false;
        };

        docsGenerationCommands = ''
          export HOME="$TMPDIR/home"
          mkdir -p "$HOME"

          server --print-openapi > openapi.json
          cp openapi.json mcp-v8-client/openapi.json
          python3 scripts/generate_http_api_reference.py
          generate-cli-markdown > site-docs/reference/cli-flags.md
          generate-mcp-tools-markdown > site-docs/reference/mcp-tools.md
        '';
      in {
        devShells.default = import ./shell.nix { inherit pkgs rustToolchain rustyV8Archive; };
        devShells.fuzz = import ./shell.nix { inherit pkgs rustyV8Archive; rustToolchain = rustNightly; };

        # SQLite compiled to WASM via Emscripten — used by the sqlite-wasm example.
        packages.sqlite-wasm = import ./nix/sqlite-wasm.nix { inherit pkgs; };
        packages.docs-tools = docsTools;

        packages.default = rustPlatform.buildRustPackage {
          pname = "mcp-js-server";
          version = "0.1.0";
          src = ./server;

          # Use a local copy of fetchCargoVendor so the staging helper fetches
          # crates from the registry CDN instead of the crates.io API endpoint.
          cargoDeps = serverCargoDeps;

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

        packages.docs = pkgs.stdenvNoCC.mkDerivation {
          pname = "mcp-js-docs";
          version = "0.1.0";
          src = self;

          nativeBuildInputs = [
            docsTools
            docsPython
          ];

          dontConfigure = true;
          strictDeps = true;

          buildPhase = ''
            runHook preBuild

            ${docsGenerationCommands}
            python3 -m mkdocs build --strict

            runHook postBuild
          '';

          installPhase = ''
            runHook preInstall
            cp -R site "$out"
            runHook postInstall
          '';
        };
      }
    )) // {
      inherit nixosModules;

      # NixOS integration test – run with:
      #   nix build .#checks.x86_64-linux.cluster-test
      checks.x86_64-linux =
        let
          pkgs = import nixpkgs {
            system = "x86_64-linux";
            overlays = [ rust-overlay.overlays.default ];
          };
          docsPython = pkgs.python3.withPackages (
            ps: with ps; [
              mkdocs
              mkdocs-mermaid2-plugin
            ]
          );
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
          exec-opa-test = pkgs.nixosTest (import ./tests/nixos/exec-opa.nix {
            inherit pkgs;
            mcp-js = self.packages.x86_64-linux.default;
          });
          docs-generated-check = pkgs.stdenvNoCC.mkDerivation {
            pname = "mcp-js-docs-generated-check";
            version = "0.1.0";
            src = self;

            nativeBuildInputs = [
              self.packages.x86_64-linux.docs-tools
              docsPython
            ];

            dontConfigure = true;
            strictDeps = true;

            buildPhase = ''
              runHook preBuild

              cp -R --no-preserve=mode,ownership "$src"/. repo
              chmod -R u+w repo
              cd repo

              cp openapi.json "$TMPDIR/openapi.json.orig"
              cp mcp-v8-client/openapi.json "$TMPDIR/client-openapi.json.orig"
              cp site-docs/reference/http-api.md "$TMPDIR/http-api.md.orig"
              cp site-docs/reference/cli-flags.md "$TMPDIR/cli-flags.md.orig"
              cp site-docs/reference/mcp-tools.md "$TMPDIR/mcp-tools.md.orig"

              export HOME="$TMPDIR/home"
              mkdir -p "$HOME"

              server --print-openapi > openapi.json
              cp openapi.json mcp-v8-client/openapi.json
              python3 scripts/generate_http_api_reference.py
              generate-cli-markdown > site-docs/reference/cli-flags.md
              generate-mcp-tools-markdown > site-docs/reference/mcp-tools.md

              diff -u "$TMPDIR/openapi.json.orig" openapi.json
              diff -u "$TMPDIR/client-openapi.json.orig" mcp-v8-client/openapi.json
              diff -u "$TMPDIR/http-api.md.orig" site-docs/reference/http-api.md
              diff -u "$TMPDIR/cli-flags.md.orig" site-docs/reference/cli-flags.md
              diff -u "$TMPDIR/mcp-tools.md.orig" site-docs/reference/mcp-tools.md

              runHook postBuild
            '';

            installPhase = ''
              runHook preInstall
              mkdir -p "$out"
              touch "$out/passed"
              runHook postInstall
            '';
          };
          docs-browser-test = pkgs.nixosTest (import ./tests/nixos/docs-browser.nix {
            inherit pkgs;
            docsSite = self.packages.x86_64-linux.docs;
          });
        };
    };
}
