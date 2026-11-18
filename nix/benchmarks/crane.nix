# Experimental only: production packages/default dev shell remain unchanged.
{ rustToolchain, rustyV8Archive, system, sourceInputs }:
let
  platform = builtins.getFlake "github:anthropics/cargo-nix-plugin/00f311abab999b4ad4c7f3fa3d85bb17beb91aee";
  pkgs = import platform.inputs.nixpkgs { inherit system; };
  crane = builtins.getFlake "github:ipetkov/crane/95e62def851e9d650f0eab63cb4f59a3306e20bd";
  craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchain;
  src = pkgs.lib.cleanSource ../..;
  rustcWrapper = pkgs.writeShellScript "benchmark-rustc-wrapper" ''
    exec ${pkgs.python3}/bin/python3 ${./crane-wrapper.py} rustc "$@"
  '';
  rustdocWrapper = pkgs.writeShellScript "benchmark-rustdoc-recorder" ''
    exec ${pkgs.python3}/bin/python3 ${./crane-wrapper.py} rustdoc "$@"
  '';
  common = {
    inherit src;
    pname = "mcp-js-crane-benchmark";
    version = "0.1.0";
    cargoExtraArgs = "--locked -p server";
    CARGO_PROFILE = "dev";
    CARGO_INCREMENTAL = "0";
    CARGO_TARGET_DIR = "target";
    RUSTY_V8_ARCHIVE = "${rustyV8Archive}";
    LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
    nativeBuildInputs = with pkgs; [ clang git llvmPackages.bintools pkg-config cmake perl python3 ];
    buildInputs = with pkgs; [ openssl ];
    # The root manifest remains authoritative: [profile.dev] debug=line-tables-only.
    doStrip = false;
  };
  dummySrc = craneLib.mkDummySrc {
    inherit src;
    # URL is a patched local dependency of registry crates, so an empty dummy
    # API cannot satisfy e.g. reqwest. Preserve only that actual local subtree.
    extraDummyScript = ''
      rm -rf "$out/server/vendor/url"
      mkdir -p "$out/server/vendor"
      cp -R ${../../server/vendor/url} "$out/server/vendor/url"
    '';
  };
  deps = craneLib.buildDepsOnly (common // {
    inherit dummySrc;
    # No running tests: include dev dependency compilation only.
    cargoTestExtraArgs = "--no-run";
  });
  tests = craneLib.mkCargoDerivation (common // {
    cargoArtifacts = deps;
    RUSTC_WRAPPER = rustcWrapper;
    cargoVendorDir = deps.cargoVendorDir;
    doCheck = false;
    doInstallCargoArtifacts = false;
    buildPhaseCargoCommand = ''
      export BENCHMARK_BUILD_ROOT="$PWD"
      export BENCHMARK_RUNTIME_SOURCE=${src}
      export BENCHMARK_RUNTIME_OUTPUT="$out"
      export BENCHMARK_REAL_RUSTDOC=${rustToolchain}/bin/rustdoc
      mkdir -p "$out/share"
      cargo tree --locked -p server --edges normal,build,dev --prefix none --format '{p}|{f}' > "$out/share/cargo-features.txt"
      cargo test --locked -p server --no-run --message-format=json > cargo-artifacts.jsonl
      # Record the doctest command without running/caching its verdict.
      RUSTDOC=${rustdocWrapper} cargo test --locked -p server --doc
      test -s "$out/doctests.jsonl"
    '';
    installPhaseCommand = ''
      ${pkgs.python3}/bin/python3 ${./crane-install.py} \
        cargo-artifacts.jsonl "$out" ${src} "$CARGO_TARGET_DIR"
      mv "$CARGO_TARGET_DIR" "$out/target"
    '';
    passthru = { inherit deps; };
  });
  runner = pkgs.writeShellScriptBin "run-benchmark-tests" ''
    unset LD_LIBRARY_PATH
    export PATH=${pkgs.lib.makeBinPath [ rustToolchain pkgs.bash pkgs.coreutils pkgs.curl pkgs.git pkgs.python3 pkgs.deno pkgs.openssl pkgs.pkg-config ]}:"$PATH"
    exec ${pkgs.python3}/bin/python3 ${./run-crane-tests.py} ${tests}
  '';
in {
  benchmark-nix = pkgs.nixVersions.nix_2_34;
  benchmark-config = pkgs.writeText "benchmark-config.json" (builtins.toJSON {
    implementation = "crane";
    rustc_version = rustToolchain.version;
    nix_version = pkgs.nixVersions.nix_2_34.version;
    nixpkgs_revision = platform.inputs.nixpkgs.rev;
    debug = (builtins.fromTOML (builtins.readFile ../../Cargo.toml)).profile.dev.debug;
    cargo_lock_sha256 = builtins.hashString "sha256" (builtins.readFile ../../Cargo.lock);
    profile = "dev";
    upstream_revision = "95e62def851e9d650f0eab63cb4f59a3306e20bd";
    rustc_flags = [ ];
    native_compiler_version = pkgs.stdenv.cc.version;
  });
  benchmark-source-roots = pkgs.writeText "benchmark-source-roots" (
    pkgs.lib.concatStringsSep "\n" (
      [ crane.outPath platform.outPath platform.inputs.nixpkgs.outPath ] ++ sourceInputs
    ) + "\n"
  );
  benchmark-deps = deps;
  benchmark-tests = tests;
  benchmark-runner = runner;
}
