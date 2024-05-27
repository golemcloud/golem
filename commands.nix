{ pkgs ? import <nixpkgs> {}
, prefix ? "glm"
# this default "fetch" is for nix-build but we will pass here the golem-examples
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
    welcome = ''
      ${pkgs.figlet}/bin/figlet 'golem dev shell'
      echo 'press ${prefix}-<TAB><TAB> to see all the commands'
    '';
    git = ''${pkgs.git}/bin/git "$@"'';

    git-project-path = ''${self.git} rev-parse --show-toplevel'';

    add-golem-examples-symlink-info = ''
        echo 'this command will symlink the golem-example project into the current folder.'
        echo 'we have to do this due to issue with nix not being able to load correctly custom build script dependencies'
    '';
    add-golem-examples-symlink = ''
        ln -sf ${golem-examples} golem-examples
    '';
    update-golem-examples-from-lock =''
        nix flake lock --update-input golem-examples
    '';

    create-patches = ''
     ${self.git} diff $(${self.git-project-path})/Cargo.lock $(${self.git-project-path})/golem-cli/Cargo.toml > nixDeps.patch
     ${self.git} diff $(${self.git-project-path})/golem-client/build.rs > fixOldSyntax.patch
    '';
    apply-patches = ''
       ${self.git} apply *.patch 
    '';
    revert-patches = ''
       ${self.git} apply -R *.patch 
    '';

    build-with-nix-deps-golem-cli = ''
        ${pkgs.cargo}/bin/cargo build -p golem-cli
    '';
    
    update-deps = ''
        ${self.update-golem-examples-from-lock} && \
        ${self.apply-patches} && \
        ${self.add-golem-examples-symlink} && \
        ${self.build-with-nix-deps-golem-cli} && \
        ${self.create-patches} && \
        ${self.revert-patches} 
    '';
    default = self.update-deps;

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
