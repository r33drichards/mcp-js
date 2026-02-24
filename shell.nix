{ pkgs ? import <nixpkgs> {}, rustToolchain ? null }:
  pkgs.mkShell rec {
    buildInputs = with pkgs; [
      (if rustToolchain != null then rustToolchain else rustup)
      clang
      llvmPackages.bintools
      deno
      cacert
      openssl
      pkg-config
      # Provides libstdc++.so.6 needed by ASAN-instrumented fuzz targets
      stdenv.cc.cc.lib
    ];
    # https://github.com/rust-lang/rust-bindgen#environment-variables
    LIBCLANG_PATH = pkgs.lib.makeLibraryPath [ pkgs.llvmPackages_latest.libclang.lib ];
    shellHook = ''
      export PATH=$PATH:''${CARGO_HOME:-~/.cargo}/bin
      # Set SSL certificate path
      export SSL_CERT_FILE=${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt
      # Set library path for OpenSSL
      export LD_LIBRARY_PATH=${pkgs.lib.makeLibraryPath buildInputs}:$LD_LIBRARY_PATH
      '';
    # Add precompiled library to rustc search path
    RUSTFLAGS = (builtins.map (a: ''-L ${a}/lib'') [
      # add libraries here (e.g. pkgs.libvmi)
    ]);


  }
