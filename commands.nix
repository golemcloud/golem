{ pkgs ? import <nixpkgs> {}
, prefix ? "glm"
# this default fetch is for nix-build but we will pass here the golem-examples
# from flake to make it easier to update
, golem-examples ? pkgs.fetchFromGitHub { 
            owner = "golemcloud";
            repo = "golem-examples";
            rev = "5165dcc7e3cbfa09f752caa96a869d284ec169aa";
            hash = "sha256-7uWvvrRpIo4euFgP0TG4PEXENl4+Wgd8ckPpYnAwQbw=";
        }
}:
let 

  commands = pkgs.lib.fix (self: pkgs.lib.mapAttrs pkgs.writeShellScript
  {
    sd = ''${pkgs.sd}/bin/sd "$@"'';

    welcome = ''
      ${pkgs.figlet}/bin/figlet 'golem dev shell'
      echo 'press ${prefix}-<TAB><TAB> to see all the commands'
    '';

    git-project-path = ''${pkgs.git}/bin/git rev-parse --show-toplevel'';

    replace-nix-deps-in-tomls-info = ''
        echo 'this command will replace golem-examples in Cargo.toml to come from nix.'
        echo 'we have to do this due to issue with nix not being able to load correctly custom build script dependencies'
    '';
    replace-nix-deps-in-tomls = ''
        ${self.sd} '(golem-examples\s*=).*' '$1 { path = "${golem-examples}" }' golem-cli/Cargo.toml
    '';

    backup-lock = ''cp Cargo.lock Cargo.lock.backup'';
    backup-toml = ''cp Cargo.toml Cargo.toml.backup'';

    build-with-nix-deps-golem-cli = ''
        nix shell -c "cargo build -p golem-cli"
    '';
    create-nix-cargo-lock = ''
        
    '';




    # default = "ls commands.nix | ${self.entr} -r nix run .#start";
  });
in pkgs.symlinkJoin rec {
  name = prefix;
  passthru.set = commands;
  passthru.bin = pkgs.lib.mapAttrs (name: command: pkgs.runCommand "${prefix}-${name}" {} '' 
    mkdir -p $out/bin
    ln -sf ${command} $out/bin/${
        if name == "default" then prefix else prefix+"-"+name
    }
  '') commands;
  paths = pkgs.lib.attrValues passthru.bin;
}
