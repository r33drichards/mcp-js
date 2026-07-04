# Offline Nix build of the Aeneas translator (and its charon-ml / easy_logging
# dependencies) from a pinned nixpkgs, avoiding the upstream flake's FStar +
# static-build inputs that need network access unavailable in a locked-down CI.
#
# Provide local checkouts of the three git sources (all buildable offline once
# cloned):
#   nixpkgsSrc      — a nixpkgs checkout/tarball (tested: nixos-25.05, ocaml 5.2.1)
#   charonSrc       — https://github.com/AeneasVerif/charon      (rev 19e3f85)
#   aeneasSrc       — https://github.com/AeneasVerif/aeneas      (rev 45061fa)
#   easyLoggingSrc  — https://github.com/sapristi/easy_logging   (tag v0.8.2)
#
# Example:
#   nix-build aeneas.nix --arg charonSrc ~/src/charon \
#     --arg aeneasSrc ~/src/aeneas --arg easyLoggingSrc ~/src/easy_logging
#
# Note: aeneas' src/Parallel.ml must be replaced with a sequential stub, because
# domainslib is unbuildable against this nixpkgs' saturn (a version skew). See
# README.md.
{ nixpkgsSrc ? <nixpkgs>
, charonSrc
, aeneasSrc
, easyLoggingSrc
}:
let
  pkgs = import nixpkgsSrc {};
  lib = pkgs.lib;
  ocamlPackages = pkgs.ocaml-ng.ocamlPackages_5_2;
  ocaml-ng = pkgs.ocaml-ng;

  easy_logging = ocamlPackages.buildDunePackage {
    pname = "easy_logging";
    version = "0.8.2";
    src = easyLoggingSrc;
    buildInputs = [ ocamlPackages.calendar ];
  };

  # charon-ml source: only the charon-ml dir + top-level dune-project.
  charonMlSrc = lib.cleanSourceWith {
    src = charonSrc;
    filter = path: type:
      (lib.hasPrefix (toString (charonSrc + "/charon-ml")) path)
      || (lib.hasPrefix (toString (charonSrc + "/dune-project")) path);
  };

  charon-name_matcher_parser = ocamlPackages.buildDunePackage {
    pname = "name_matcher_parser";
    version = "0.1.0";
    duneVersion = "3";
    src = charonMlSrc;
    nativeBuildInputs = [ ocamlPackages.menhir ];
    propagatedBuildInputs = with ocamlPackages; [ ppx_deriving visitors zarith menhirLib ];
  };

  charon-ml = ocamlPackages.buildDunePackage {
    pname = "charon";
    version = "0.1.0";
    duneVersion = "3";
    src = charonMlSrc;
    doCheck = false;
    propagatedBuildInputs = with ocamlPackages; [
      core ppx_deriving visitors easy_logging zarith yojson calendar
      charon-name_matcher_parser unionFind
      ocaml-ng.ocamlPackages_4_14.ppx_tools
    ];
    preBuild = ''
      sed -i 's#(glob_files_rec [^)]*)##' charon-ml/tests/dune || true
    '';
  };

  aeneas = ocamlPackages.buildDunePackage {
    pname = "aeneas";
    version = "0.1";
    duneVersion = "3";
    src = aeneasSrc;
    doCheck = false;
    nativeBuildInputs = [ ocamlPackages.menhir ];
    propagatedBuildInputs = with ocamlPackages; [
      charon-ml core_unix ocamlgraph progress
      ppx_deriving ppx_deriving_yojson visitors ppxlib
    ];
    # Replace the domainslib-based Parallel module with a sequential stub, and
    # drop domainslib from the dune libraries. domainslib 0.5.1 does not compile
    # against this nixpkgs' saturn 0.5.0 (`Foreign_queue.pop` unbound), and
    # translation correctness does not depend on parallelism.
    postPatch = ''
      cat > src/Parallel.ml <<'EOF'
let parallel_map (f : 'a -> 'b) (ls : 'a list) : 'b list = List.map f ls
let parallel_filter_map (f : 'a -> 'b option) (ls : 'a list) : 'b list =
  List.filter_map f ls
let parallel_map_chunks = parallel_map
let parallel_filter_map_chunks = parallel_filter_map
let parallel_map_pool = parallel_map
let parallel_filter_map_pool = parallel_filter_map
EOF
      sed -i '/^  domainslib$/d' src/dune
    '';
    # Only build the aeneas library + main binary, skip tests/test_runner.
    buildPhase = ''
      runHook preBuild
      cd src && dune build --profile release main.exe
      runHook postBuild
    '';
    installPhase = ''
      runHook preInstall
      mkdir -p $out/bin $out/backends
      cp -f _build/default/main.exe $out/bin/aeneas
      cp -rf ../backends/lean $out/backends/lean
      runHook postInstall
    '';
  };
in
aeneas
