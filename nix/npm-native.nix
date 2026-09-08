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

  buildPhase = ''
    runHook preBuild
    cd node
    npm run build
    runHook postBuild
  '';
  installPhase = ''
    runHook preInstall
    mkdir -p "$out"
    # Validate this exact tarball outside Nix before release.
    npm pack --ignore-scripts --pack-destination "$out"
    runHook postInstall
  '';
}
