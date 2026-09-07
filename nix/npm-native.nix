{ pkgs, src }:

pkgs.buildNpmPackage {
  pname = "mcp-js-node-npm";
  version = "0.1.0";
  inherit src;
  npmRoot = "node";

  npmDeps = pkgs.importNpmLock {
    npmRoot = ../node;
  };
  npmConfigHook = pkgs.importNpmLock.npmConfigHook;

  nativeBuildInputs = with pkgs; [
    binutils
    patchelf
  ];

  npmBuildScript = "build";
  installPhase = ''
    runHook preInstall
    mkdir -p "$out"
    npm pack --pack-destination "$out"
    runHook postInstall
  '';
}
