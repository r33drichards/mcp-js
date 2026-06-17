{ pkgs ? import <nixpkgs> {}, rustToolchain ? null, rustyV8Archive ? null }:
  pkgs.mkShell rec {
    buildInputs = with pkgs; [
      (if rustToolchain != null then rustToolchain else rustup)
      clang
      llvmPackages.bintools
      deno
      cacert
      openssl
      pkg-config
      
      stdenv.cc.cc.lib
    ];
    
    LIBCLANG_PATH = pkgs.lib.makeLibraryPath [ pkgs.llvmPackages_latest.libclang.lib ];
    shellHook = ''
      export PATH=$PATH:''${CARGO_HOME:-~/.cargo}/bin
      # Set SSL certificate path
      export SSL_CERT_FILE=${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt
      # Set library path for OpenSSL
      export LD_LIBRARY_PATH=${pkgs.lib.makeLibraryPath buildInputs}:$LD_LIBRARY_PATH
    '' + pkgs.lib.optionalString (rustyV8Archive != null) ''
      # Point v8 build.rs to the pre-fetched static library
      export RUSTY_V8_ARCHIVE=${rustyV8Archive}
    '';
    
    RUSTFLAGS = (builtins.map (a: ''-L ${a}/lib'') [
      
    ]);


  }
