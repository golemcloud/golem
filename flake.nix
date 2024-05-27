{
  inputs.golem-examples.url = "github:golemcloud/golem-examples";
  inputs.golem-examples.flake = false;

  outputs = { self, nixpkgs, golem-examples }:
  let system = "x86_64-linux";
  pkgs = import nixpkgs { inherit system;};
  commands = import ./commands.nix { inherit pkgs golem-examples;};
  in
  {
    # packages.${system}.update = import ./default.nix { inherit pkgs golem-examples; };
    packages.${system}.default = import ./default.nix { inherit pkgs golem-examples;
        
        # updateTOML = commands.set.replace-nix-deps-in-tomls;
    };
    devShells.${system}.default = pkgs.mkShell {
        name = "shell";
        buildInputs = [
            commands

            pkgs.sd

            pkgs.cargo
            pkgs.rustc
            pkgs.rustPlatform.cargoSetupHook
            pkgs.rustPlatform.maturinBuildHook
            pkgs.openssl.dev
            pkgs.rustPlatform.bindgenHook
            pkgs.pkg-config
            pkgs.openssl
      ];

      PROTOC = "${pkgs.protobuf}/bin/protoc";
      shellHook = commands.set.welcome;
    };
  };
}
