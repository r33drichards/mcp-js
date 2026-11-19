{ pkgs, rustToolchain, rustyV8Version }:
let
  target = "x86_64-unknown-linux-musl";
  cross = pkgs.pkgsCross.musl64;
  cc = cross.stdenv.cc;
  tools = "${cc}/bin/${cc.targetPrefix}";
  archiveBase = "https://github.com/denoland/rusty_v8/releases/download/v${rustyV8Version}";
in
pkgs.mkShell {
  nativeBuildInputs = [
    (rustToolchain.override { targets = [ target ]; })
    cc
    pkgs.cmake
    pkgs.ninja
    pkgs.perl
    pkgs.pkg-config
  ];

  CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER = "${tools}cc";
  # A UniFFI cdylib must use the same dynamic musl libc as its host process.
  CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_RUSTFLAGS = "-C target-feature=-crt-static";
  CC_x86_64_unknown_linux_musl = "${tools}cc";
  CXX_x86_64_unknown_linux_musl = "${tools}c++";
  AR_x86_64_unknown_linux_musl = "${cc.bintools}/bin/${cc.targetPrefix}ar";

  RUSTY_V8_ARCHIVE = pkgs.fetchurl {
    url = "${archiveBase}/librusty_v8_release_${target}.a.gz";
    hash = "sha256-aPKE+4GEufno1+I7jyZ12+Rk9D93wL8VL3Bb9+rSlVg=";
  };
  RUSTY_V8_SRC_BINDING_PATH = pkgs.fetchurl {
    url = "${archiveBase}/src_binding_release_${target}.rs";
    hash = "sha256-Pk1f7Nvg+YpgF+adfKglGad/sDLF615M2+zdPi7FGdU=";
  };
}
