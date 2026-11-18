# Experimental only: production packages and the project lockfile are unchanged.
{ system, rustToolchain, rustyV8Archive, source, projectInputs }:
let
  pluginFlake = builtins.getFlake "github:anthropics/cargo-nix-plugin/00f311abab999b4ad4c7f3fa3d85bb17beb91aee";
  pkgs = import pluginFlake.inputs.nixpkgs { inherit system; };
  lib = pkgs.lib;
  # The plugin ABI must match this exact runtime from its own locked nixpkgs.
  nix = pkgs.nixVersions.nix_2_34;
  # swc_core's sparse index is >16 MiB. ureq's default body limit is
  # 10 MiB and upstream prefetch only reports failure if every job fails.
  # Keep the resolver/plugin unchanged; seed a complete cache with a bounded
  # 64 MiB prefetch read and retain the independent --check phase.
  prefetch = pluginFlake.packages.${system}.cargo-nix-prefetch.overrideAttrs (old: {
    postPatch = (old.postPatch or "") + ''
      substituteInPlace src/registry.rs --replace-fail \
        '.read_to_vec()' '.with_config().limit(64 * 1024 * 1024).read_to_vec()'
    '';
  });
  # Native lockfile mode does not discover root [patch.*] path sources.
  # Add source discovery only; preserve application manifests and features.
  resolverRustPlatform = pkgs.rustPlatform // {
    buildRustPackage = args: pkgs.rustPlatform.buildRustPackage (args // {
      patches = (args.patches or [ ]) ++ [ ./cargo-nix-root-path-patches.patch ];
    });
  };
  resolverPlugin = pluginFlake.packages.${system}.cargo-nix-plugin.override {
    rustPlatform = resolverRustPlatform;
  };
  plugin = pkgs.buildEnv {
    name = "cargo-nix-benchmark-bootstrap";
    paths = [
      resolverPlugin
      pluginFlake.packages.${system}.build-rust-crate-bin
      prefetch
    ];
  };
  driver = ./cargo-nix-driver.py;
  fullSource = lib.cleanSource source;
  nativeInputs = with pkgs; [ clang git llvmPackages.bintools pkg-config cmake perl python3 ];
  runtimeInputs = with pkgs; [ bash coreutils curl git python3 deno openssl pkg-config rustToolchain ];
  serverRust = pkgs.symlinkJoin {
    name = "benchmark-server-rust-with-durable-fixtures";
    paths = [ rustToolchain ];
    nativeBuildInputs = [ pkgs.makeWrapper ];
    postBuild = ''
      rm "$out/bin/rustc" "$out/bin/rustdoc"
      makeWrapper ${pkgs.python3}/bin/python3 "$out/bin/rustc" \
        --add-flags "${driver} rustc ${rustToolchain}/bin/rustc ${fullSource}"
      makeWrapper ${pkgs.python3}/bin/python3 "$out/bin/rustdoc" \
        --add-flags "${driver} rustdoc ${rustToolchain}/bin/rustdoc ${fullSource}"
    '';
  };
  # Package-local sources stop a harmless server edit from invalidating every
  # workspace dependency. Keep the root manifests for inherited package fields.
  localSrc = member: {
    src = lib.cleanSourceWith {
      src = source;
      filter = path: type:
        let rel = lib.removePrefix (toString source + "/") (toString path);
        in rel == "Cargo.toml" || rel == "Cargo.lock"
          || (member == "server" && (rel == "tools" || lib.hasPrefix "tools/" rel))
          || rel == member || lib.hasPrefix (member + "/") rel
          || (type == "directory" && lib.hasPrefix (rel + "/") member);
    };
    workspace_member = if member == "" then "." else member;
  };
  cargoNix = pluginFlake.lib {
    inherit pkgs localSrc;
    src = fullSource;
    buildRustCrateForPkgs = cratePkgs: _base: args:
      (cratePkgs.callPackage "${pluginFlake.outPath}/nix/build-rust-crate" {
        buildRustCrateBin = pluginFlake.packages.${system}.build-rust-crate-bin;
        rustc = rustToolchain;
        cargo = rustToolchain;
        defaultMold = null;
      }) (args // {
        rust = if args.crateName == "server" then serverRust else rustToolchain;
        cargo = rustToolchain;
        release = false;
        debugInfo = 1;
        codegenUnits = 256;
        extraRustcOpts = [ "-C" "debuginfo=line-tables-only" ];
        extraRustcOptsForBuildRs = [ "-C" "debuginfo=line-tables-only" ];
        nativeBuildInputs = nativeInputs;
        buildInputs = [ pkgs.openssl ];
        LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
        RUSTY_V8_ARCHIVE = "${rustyV8Archive}";
      });
  };
  # Upstream invokes doctest after all compiles. The server-specific rustdoc
  # wrapper records the command, so no test verdict is cached in this output.
  resolution = pkgs.writeText "cargo-nix-benchmark-resolved-graph.json" (builtins.toJSON cargoNix.resolved);
  config = pkgs.writeText "cargo-nix-benchmark-config.json" (builtins.toJSON {
    implementation = "cargo-nix-plugin";
    rustc_version = rustToolchain.version;
    nixpkgs_revision = pluginFlake.inputs.nixpkgs.rev;
    debug = "line-tables-only";
    profile = "dev";
    codegen_units = 256;
    cargo_lock_sha256 = builtins.hashFile "sha256" (source + "/Cargo.lock");
    upstream_revision = "00f311abab999b4ad4c7f3fa3d85bb17beb91aee";
    prefetch_patch = "ureq-body-limit-64MiB";
    resolver_patch = "discover-root-patch-paths";
    resolver_scope = "all-workspace-roots";
    feature_resolution = "resolver2-host-target-unified-superset";
    feature_parity = "differs-local-audit-awaiting-ci";
    local_feature_differences = [
      "reqwest 0.12.28: plugin adds blocking"
      "schemars 0.8.22: plugin adds chrono,uuid1"
      "chrono 0.4.44: Cargo reports js-sys,wasm-bindgen,windows-link"
    ];
  });
  tests = cargoNix.workspaceMembers.server.doctest.overrideAttrs (old: {
    postBuild = (old.postBuild or "") + ''
      ${pkgs.python3}/bin/python3 ${driver} example
    '';
    postInstall = (old.postInstall or "") + ''
      ${pkgs.python3}/bin/python3 ${driver} manifest
      cp ${resolution} "$out/share/benchmark-resolved-graph.json"
    '';
  });
  runner = pkgs.writeShellApplication {
    name = "run-benchmark-tests";
    inherit runtimeInputs;
    text = ''
      work=$(mktemp -d)
      trap 'rm -rf "$work"' EXIT
      export CARGO_TARGET_TMPDIR="$work/target-tmp"
      mkdir -p "$CARGO_TARGET_TMPDIR"
      export RUST_BACKTRACE=1
      unset LD_LIBRARY_PATH
      cd ${fullSource}/server
      python3 ${driver} run ${tests}
    '';
  };
  sourceRoots = pkgs.writeText "cargo-nix-benchmark-evaluation-roots" (
    let
      collect = input: [ input.outPath ] ++ lib.concatMap collect (lib.attrValues (input.inputs or { }));
    in lib.concatStringsSep "\n" (lib.concatMap collect ([ pluginFlake ] ++ projectInputs)) + "\n"
  );
in {
  inherit nix plugin tests runner sourceRoots config;
}
