{ pkgs ? import <nixpkgs> {} }:

pkgs.rustPlatform.buildRustPackage rec {
  pname = "mcp-v8";
  version = "0.1.0";

  src = ./.;

  # Path to the Cargo.toml in the server subdirectory
  cargoRoot = "server";

  # Use the Cargo.lock file directly
  cargoLock = {
    lockFile = ./server/Cargo.lock;
    # Hash for git dependencies
    outputHashes = {
      "rmcp-0.1.5" = "sha256-3IPIlk1zIIemtJ4YeWgV4Qe3NyyR0I/nvDeqDebxyl4=";
    };
  };

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

  # Build in release mode
  buildType = "release";

  # The binary is named "server" in Cargo.toml, but we want to rename it
  postInstall = ''
    mv $out/bin/server $out/bin/mcp-v8
  '';

  meta = with pkgs.lib; {
    description = "MCP V8 JavaScript execution server";
    homepage = "https://github.com/your-repo/mcp-v8";
    license = licenses.mit;
    maintainers = [ ];
  };
}
