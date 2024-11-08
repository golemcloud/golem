{ pkgs ? import <nixpkgs> {} }:
with pkgs;
mkShell {
    nativeBuildInputs = [ rustup protobuf openssl.dev pkg-config ];
}
