{ pkgs }:

let
  lib = pkgs.lib;
  llvm = pkgs.llvmPackages_latest;
  rustToolchain = pkgs.rust-bin.stable."1.89.0".default.override {
    extensions = [ "rustfmt" ];
  };
  rustPlatform = pkgs.makeRustPlatform {
    cargo = rustToolchain;
    rustc = rustToolchain;
  };
  chromiumRustToolchain = pkgs.symlinkJoin {
    name = "rusty-v8-rust-toolchain";
    paths = [ rustToolchain llvm.libclang.lib ];
  };
  clangBasePath = pkgs.symlinkJoin {
    name = "rusty-v8-clang-toolchain";
    paths = [ llvm.clang-unwrapped.lib llvm.clang llvm.llvm llvm.lld ];
  };
in
rustPlatform.buildRustPackage {
  pname = "rusty-v8-shared";
  version = "145.0.0-e6a88b35";

  src = pkgs.fetchFromGitHub {
    owner = "denoland";
    repo = "rusty_v8";
    rev = "e6a88b35dd3d7f2849a0df33a71d338701c55316";
    fetchSubmodules = true;
    hash = "sha256-uFB5Ao92c4tTTpEli5se8I9fvBrNHrDV3sbxJDokp/M=";
  };
  cargoHash = lib.fakeHash;

  nativeBuildInputs = [ llvm.clang pkgs.python3 pkgs.pkg-config llvm.lld ];
  buildInputs = [ pkgs.glib pkgs.icu ];

  postPatch = ''
    substituteInPlace build.rs \
      --replace-fail "  download_rust_toolchain();" ""
    rm -rf third_party/rust-toolchain
    ln -s ${chromiumRustToolchain} third_party/rust-toolchain
  '';

  env = {
    V8_FROM_SOURCE = "1";
    PYTHON = "python3";
    NINJA = lib.getExe pkgs.ninja;
    GN = lib.getExe pkgs.gn;
    RUSTC_BOOTSTRAP = "1";
    LIBCLANG_PATH = lib.makeLibraryPath [ llvm.libclang ];
    CLANG_BASE_PATH = clangBasePath;
    GN_ARGS = "v8_monolithic=true v8_monolithic_for_shared_library=true";
    EXTRA_GN_ARGS = lib.concatStringsSep " " [
      "use_sysroot=false"
      "clang_version=\"${lib.versions.major llvm.clang.version}\""
      "rustc_version=\"${rustToolchain.version}\""
      "rust_sysroot_absolute=\"${chromiumRustToolchain}\""
      "rust_bindgen_root=\"${chromiumRustToolchain}\""
    ];
  };

  doCheck = false;
  requiredSystemFeatures = [ "big-parallel" ];

  installPhase = ''
    runHook preInstall
    mkdir -p "$out/lib"
    cp target/*/release/gn_out/obj/librusty_v8.a "$out/lib/librusty_v8.a"
    cp target/*/release/gn_out/src_binding.rs "$out/src_binding.rs"
    runHook postInstall
  '';

  meta.platforms = [ "x86_64-linux" ];
}
