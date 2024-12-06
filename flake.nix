{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
    rust-overlay.inputs.nixpkgs.follows = "nixpkgs";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      self,
      rust-overlay,
      flake-utils,
      nixpkgs,
    }:
    let

      golemDevEnv =
        rust-bin: target:
        {
          mkShell,
          pkg-config,
          qemu,
          openssl,
          stdenv,
          protobuf,
          fontconfig,
          libiconv,
          lib,
        }:
        mkShell {
          nativeBuildInputs =
            [
              rust-bin.stable.latest.default
              pkg-config
              protobuf
            ]
            + lib.optionals stdenv.buildPlatform.isDarwin [
              libiconv
            ];

          depsBuildBuild = [ qemu ];
          buildInputs = [
            openssl
            fontconfig
          ];

          env =
            if target == "aarch64-linux" then
              {
                CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER = "${stdenv.cc.targetPrefix}cc";
                CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUNNER = "qemu-aarch64";
                HOST_CC = "${stdenv.cc.nativePrefix}cc";
                TARGET_CC = "${stdenv.cc.targetPrefix}cc";
              }
            else
              { };
        };
    in
    flake-utils.lib.eachDefaultSystem (system: {
      devShells.default =
        let
          target = system;
          overlays = [ (import rust-overlay) ];
          pkgs = import nixpkgs {
            inherit system overlays;
          };
        in
        pkgs.callPackage (golemDevEnv pkgs.rust-bin target) { };
      devShells.cross-arm64 =
        let
          target = "aarch64-linux";
          pkgsCross = nixpkgs.legacyPackages.${system}.pkgsCross.aarch64-multiplatform;
          rust-bin = rust-overlay.lib.mkRustBin { } pkgsCross.buildPackages;
        in
        pkgsCross.callPackage (golemDevEnv rust-bin target) { };
    });
}
