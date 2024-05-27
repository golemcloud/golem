{ pkgs? import <nixpkgs> {}
, golem-examples ? pkgs.fetchFromGitHub { 
    owner = "golemcloud";
    repo = "golem-examples";
    rev = "5165dcc7e3cbfa09f752caa96a869d284ec169aa";
    hash = "sha256-7uWvvrRpIo4euFgP0TG4PEXENl4+Wgd8ckPpYnAwQbw=";
  }
}:
pkgs.rustPlatform.buildRustPackage {
  pname = "golem-cli";
  version = "0.0.98";

  src = pkgs.lib.cleanSource ./.;

  cargoLock = {
    lockFileContents = builtins.readFile ./Cargo.lock.nix;
    # lockFile = ./Cargo.lock;
    allowBuiltinFetchGit = false;
     outputHashes = {
       "libtest-mimic-0.7.0" = "sha256-xUAyZbti96ky6TFtUjyT6Jx1g0N1gkDPjCMcto5SzxE=";
       "cranelift-bforest-0.104.0" = "sha256-veZc4s+OitjBv4ohzzgFdAxLm/J/B5NVy+RXU0hgfwQ=";
     };
  };
  preBuild = ''
    cp -r ${golem-examples} golem-examples
    mv Cargo.lock.nix Cargo.lock
    mv golem-cli/Cargo.toml.nix golem-cli/Cargo.toml
  '';

  cargoBuildFlags = [ "-p" "golem-cli" ];
  nativeBuildInputs = [
    pkgs.pkg-config

  ];

  PROTOC = "${pkgs.protobuf}/bin/protoc";

  buildInputs = [
    pkgs.openssl
  ];

  doCheck = false;
}
