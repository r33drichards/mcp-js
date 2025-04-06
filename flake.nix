{
  description = "A basic rust cli";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  inputs.flake-utils.url = "github:numtide/flake-utils";


  outputs = { self, nixpkgs, flake-utils, ... }:
    (flake-utils.lib.eachDefaultSystem
      (system:
        let
          pkgs = import nixpkgs {
            inherit system;
          };

          app = pkgs.rustPlatform.buildRustPackage {
          pname = "app";
            version = "0.0.1";
            src = ./.;

            cargoLock = {
              lockFile = ./Cargo.lock;
            };

            nativeBuildInputs = [ 
              pkgs.pkg-config 
            ];
            PKG_CONFIG_PATH = "${pkgs.openssl.dev}/lib/pkgconfig";

            buildPhase = ''
              cargo build --release
            '';

            installPhase = ''
              mkdir -p $out/bin
              cp target/release/app $out/bin/app
            '';

            # disable checkPhase
            doCheck = false;

          };
        in
        {
          app = app;
          packages.default = app;
          # devShells.default = app;
          devShells.default = import ./shell.nix { inherit pkgs; };

        })
    );
}
