{
  description = "Golem — Rust workspace dev environment and CLI";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    crane = {
      url = "github:ipetkov/crane";
    };
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay, crane }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ (import rust-overlay) ];
        };

        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [ "rust-src" "rust-analyzer" "clippy" "rustfmt" ];
          targets = [ "wasm32-wasip1" "wasm32-wasip2" ];
        };

        craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchain;

        isLinux = pkgs.stdenv.isLinux;
        isDarwin = pkgs.stdenv.isDarwin;

        commonNativeBuildInputs = [
          pkgs.pkg-config
          pkgs.protobuf
          pkgs.cmake
          pkgs.git
        ];

        commonBuildInputs = [
          pkgs.openssl
          pkgs.openssl.dev
          pkgs.zstd
          pkgs.cacert
        ] ++ pkgs.lib.optionals isDarwin [
          pkgs.darwin.apple_sdk.frameworks.Security
          pkgs.darwin.apple_sdk.frameworks.SystemConfiguration
        ];

        cargoTools = [
          pkgs.cargo-make
          pkgs.cargo-nextest
          pkgs.cargo-binstall
          pkgs.cargo-component
        ];

        wasmTools = [
          pkgs.wasm-tools
          pkgs.wit-bindgen
        ];

        # Source: keep templates/, skills/, .wit, etc. — golem-cli's build.rs
        # embeds template files, so the standard cleanCargoSource is too aggressive.
        src = pkgs.lib.cleanSourceWith {
          src = ./.;
          filter = path: type:
            let baseName = baseNameOf (toString path);
            in !(builtins.elem baseName [
              "target" "result" "result-bin"
              ".git" ".github" ".direnv"
              "logs" "data"
            ]);
          name = "golem-source";
        };

        cargoVendorDir = craneLib.vendorCargoDeps {
          cargoLock = ./Cargo.lock;
          outputHashes = {
            "git+https://github.com/golemcloud/wasmtime.git?branch=golem-wasmtime-v42.0.1#9fc55305c583e4d98edecfdab59dab5e5c3f6e1c" =
              "sha256-vJmlbdEatoVKRGNbdrrEXlOhBPKHvtqBhGytVIxMn68=";
            "git+https://github.com/golemcloud/wasm-rquickjs.git?tag=v0.2.4#6d08b6db89dcf6735b0d0d7866745e458f61d8b7" =
              "sha256-DmehvcTeasIxbYOB7DlPSJsjEsalDQ1Hobn5devqHJw=";
          };
          # Several Cargo.tomls in the wasmtime fork reference README files that
          # don't exist in the crate subdirectory; cargo package -l rejects them.
          # Materialize empty placeholders so vendoring succeeds.
          overrideVendorGitCheckout = _psLockMetadata: drv:
            drv.overrideAttrs (old: {
              preInstall = (old.preInstall or "") + ''
                find . -name Cargo.toml -print0 | while IFS= read -r -d "" cargoToml; do
                  dir=$(dirname "$cargoToml")
                  readme=$(awk -F '"' '/^[[:space:]]*readme[[:space:]]*=/ {print $2; exit}' "$cargoToml")
                  if [ -n "$readme" ] && [ ! -e "$dir/$readme" ]; then
                    mkdir -p "$dir/$(dirname "$readme")"
                    : > "$dir/$readme"
                  fi
                done
              '';
            });
        };

        commonArgs = {
          inherit src cargoVendorDir;
          pname = "golem-workspace";
          version = "0.0.0";
          strictDeps = true;
          nativeBuildInputs = commonNativeBuildInputs;
          buildInputs = commonBuildInputs;

          # golem-common's shadow build.rs requires a generated module; let it run
          # (it falls back to version "0.0.0" if `git describe` fails in the sandbox).
          OPENSSL_NO_VENDOR = "1";
          PROTOC = "${pkgs.protobuf}/bin/protoc";

          cargoExtraArgs = "--locked --workspace";
          doCheck = false;
        };

        cargoArtifacts = craneLib.buildDepsOnly commonArgs;

        golem-cli = craneLib.buildPackage (commonArgs // {
          inherit cargoArtifacts;
          pname = "golem-cli";
          cargoExtraArgs = "--locked -p golem-cli --bin golem-cli";
        });
      in
      {
        packages = {
          default = golem-cli;
          golem-cli = golem-cli;
        };

        checks = {
          inherit golem-cli;

          clippy = craneLib.cargoClippy (commonArgs // {
            inherit cargoArtifacts;
            pname = "golem-clippy";
            cargoClippyExtraArgs = "--all-targets -- --no-deps -Dwarnings";
          });

          fmt = craneLib.cargoFmt {
            inherit src;
            pname = "golem-fmt";
          };

          unit-tests = craneLib.cargoTest (commonArgs // {
            inherit cargoArtifacts;
            pname = "golem-unit-tests";
            cargoTestExtraArgs = "--lib --exclude golem-wasm-derive";
          });
        };

        devShells.default = pkgs.mkShell {
          nativeBuildInputs = commonNativeBuildInputs;
          buildInputs = commonBuildInputs ++ [ rustToolchain ] ++ cargoTools ++ wasmTools;

          shellHook = ''
            export OPENSSL_DIR="${pkgs.openssl.dev}"
            export OPENSSL_LIB_DIR="${pkgs.openssl.out}/lib"
            export OPENSSL_INCLUDE_DIR="${pkgs.openssl.dev}/include"
            export PKG_CONFIG_PATH="${pkgs.openssl.dev}/lib/pkgconfig''${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
            export PROTOC="${pkgs.protobuf}/bin/protoc"
            export RUST_SRC_PATH="${rustToolchain}/lib/rustlib/src/rust/library"
            if [ -z "''${WASI_SDK_PATH:-}" ]; then
              echo "[golem flake] WASI_SDK_PATH not set — only required to build"
              echo "              C/C++ wasm components. Install WASI SDK v25 from"
              echo "              https://github.com/WebAssembly/wasi-sdk/releases"
              echo "              and export WASI_SDK_PATH=/path/to/wasi-sdk"
            fi
            if ! command -v wasm-rquickjs >/dev/null 2>&1; then
              echo "[golem flake] wasm-rquickjs not on PATH — install with:"
              echo "              cargo binstall wasm-rquickjs@0.2.4"
            fi
          '';
        };

        formatter = pkgs.nixpkgs-fmt;
      });
}
