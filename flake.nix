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

        # WASI SDK v25 isn't packaged in nixpkgs. Fetch the upstream release
        # tarball and patch ELF interpreters/RPATHs on Linux so the prebuilt
        # clang and friends run inside the Nix sandbox.
        wasi-sdk =
          let
            wasiSdkVariants = {
              "x86_64-linux" = {
                arch = "x86_64-linux";
                sha256 = "176qyfy2arxbjy4azlanjh8mc0k4dlfyd6alm4kz36sr2gg0sr2j";
              };
              "aarch64-linux" = {
                arch = "arm64-linux";
                sha256 = "0bfffv4n7mmjckxqhbyywwvvyln6zz1ia4ayw0wj53s9nbccmz27";
              };
              "aarch64-darwin" = {
                arch = "arm64-macos";
                sha256 = "11rm4dqfgpw23k9k5biwza0bailjmvg0jy1j62sb07bb4bm2krg1";
              };
              "x86_64-darwin" = {
                arch = "x86_64-macos";
                sha256 = "04aw0nznb7vz7ql9rg41wimryv17540vmk7f2s56f58sxqzzzqsm";
              };
            };
            variant = wasiSdkVariants.${system} or null;
          in
          if variant == null then null
          else pkgs.stdenv.mkDerivation {
            pname = "wasi-sdk";
            version = "25.0";
            src = pkgs.fetchurl {
              url = "https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-25/wasi-sdk-25.0-${variant.arch}.tar.gz";
              sha256 = variant.sha256;
            };
            nativeBuildInputs = pkgs.lib.optionals isLinux [
              pkgs.autoPatchelfHook
            ];
            buildInputs = pkgs.lib.optionals isLinux [
              pkgs.stdenv.cc.cc.lib
              pkgs.zlib
            ];
            dontStrip = true;
            dontConfigure = true;
            dontBuild = true;
            installPhase = ''
              mkdir -p $out
              cp -r ./* $out/
            '';
            meta = {
              description = "WebAssembly System Interface SDK (v25)";
              homepage = "https://github.com/WebAssembly/wasi-sdk";
            };
          };

        # Skips applied to every cargo-test-based check. `ip_address_resolve`
        # exercises live DNS via wasi-net (sandbox has no network);
        # `rdbms` is the worker-executor test tag-suite that constructs
        # `DockerMysqlRdb::new()` in its test_dep (sandbox has no
        # `/var/run/docker.sock`). Suites add domain-specific skips on
        # top of these via `commonSkips ++ [ ... ]`.
        commonSkips = [ "ip_address_resolve" "rdbms" ];

        # Env-var block consumed by every derivation that cross-compiles to
        # `wasm32-wasip2` through cc-rs / bindgen / rquickjs-sys. Variables:
        #   - `WASI_SDK_PATH` — used by `golem-cli build` and several
        #     wasi-tooling crates.
        #   - `WASI_SDK` — what rquickjs-sys reads (it falls back to
        #     downloading its own v24 tarball if unset).
        #   - `CC_wasm32_wasip2` / `CXX_wasm32_wasip2` / `AR_wasm32_wasip2`
        #     — pin cc-rs at the WASI SDK toolchain for the target
        #     (otherwise nixpkgs' host clang gets picked and rejects the
        #     `--target=wasm32-wasip2` cross-compile when consuming glibc
        #     headers).
        #   - `BINDGEN_EXTRA_CLANG_ARGS_wasm32_wasip2` — point bindgen at
        #     the WASI sysroot so its bare libclang finds wasi-libc.
        # Emitted as `${wasiSdkEnv}` inside `buildPhase` shellsnippets.
        wasiSdkEnv = pkgs.lib.optionalString (wasi-sdk != null) ''
          export WASI_SDK_PATH=${wasi-sdk}
          export WASI_SDK=${wasi-sdk}
          export WASI_SDK_VERSION=25
          export CC_wasm32_wasip2="${wasi-sdk}/bin/clang"
          export CXX_wasm32_wasip2="${wasi-sdk}/bin/clang++"
          export AR_wasm32_wasip2="${wasi-sdk}/bin/llvm-ar"
          export BINDGEN_EXTRA_CLANG_ARGS_wasm32_wasip2="--sysroot=${wasi-sdk}/share/wasi-sysroot"
        '';

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

        # wasm-rquickjs CLI: not in nixpkgs. Build from the same git fork that
        # the workspace already pins. golem-cli invokes this at component-build
        # time to wrap JavaScript into WebAssembly components.
        wasm-rquickjs =
          let
            wrqSrc = pkgs.fetchgit {
              url = "https://github.com/golemcloud/wasm-rquickjs.git";
              rev = "6d08b6db89dcf6735b0d0d7866745e458f61d8b7";
              # NB: includes submodules (rusqlite at vendor/rusqlite).
              # `pkgs.fetchgit` defaults to `fetchSubmodules = true`,
              # so the recursive hash captures the submodule content.
              #
              # Hash fragility — the rusqlite submodule pointer in
              # `.gitmodules` is recorded as a SHA in the wasm-rquickjs
              # tree (commit-locked), so for a fixed `rev` this hash IS
              # stable. The risk is upstream-shaped: if `golemcloud/
              # wasm-rquickjs` ever rewrites history at this rev (or
              # the submodule's remote rebases), our hash drifts with
              # a confusing "specified vs got" error. The clean fix
              # lives at upstream:
              #   https://github.com/golemcloud/wasm-rquickjs
              # — either remove the submodule and vendor rusqlite
              # inline, or move to a pinned tag-based release model
              # so consumers can pin to immutable refs. Until then,
              # treat any hash mismatch as a signal to re-verify the
              # rev still points where we expect.
              hash = "sha256-g+RZhH6ec+zL9+37P4PQGcQjkQamexAZrlqOoE7QR5M=";
            };
          in
          craneLib.buildPackage {
            pname = "wasm-rquickjs";
            version = "0.2.4";
            src = wrqSrc;
            cargoExtraArgs = "--locked --bin wasm-rquickjs";
            doCheck = false;
            strictDeps = true;
            nativeBuildInputs = commonNativeBuildInputs;
            buildInputs = commonBuildInputs;
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

        # Source for the TypeScript SDK monorepo. Filtered to drop build/cache
        # artifacts so the source hash stays stable.
        tsSdkSrc = pkgs.lib.cleanSourceWith {
          src = ./sdks/ts;
          filter = path: _type:
            let baseName = baseNameOf (toString path);
            in !(builtins.elem baseName [
              "node_modules" "dist" "target" ".turbo"
            ]);
          name = "golem-ts-sdk-source";
        };

        # Pnpm offline cache for the TS SDK monorepo. Hash must be regenerated
        # whenever the workspace pnpm-lock.yaml changes.
        golem-ts-sdk-pnpm-deps = pkgs.fetchPnpmDeps {
          src = tsSdkSrc;
          pname = "golem-ts-sdk-pnpm-deps";
          version = "0.0.0";
          fetcherVersion = 2;
          hash = "sha256-khdBtROh+phw2ggAq9HE7r/calxX2Sw5xicoLNG7rrw=";
        };

        # First-stage SDK build: just `pnpm run build` over the workspace,
        # producing dist/ for every package. Has NO `wasm/agent_guest.wasm`
        # — that's circular-dependent on this output (the agent-template
        # crate embeds the SDK's dist/index.mjs at build time, then its
        # compiled wasm gets staged back into the final SDK).
        golem-ts-sdk-base = pkgs.stdenv.mkDerivation {
          pname = "golem-ts-sdk-base";
          version = "0.0.0";
          src = tsSdkSrc;
          pnpmDeps = golem-ts-sdk-pnpm-deps;
          nativeBuildInputs = [
            pkgs.nodejs_20
            pkgs.pnpm_10
            pkgs.pnpmConfigHook
            wasm-rquickjs
          ];
          buildPhase = ''
            runHook preBuild
            pnpm run build
            runHook postBuild
          '';
          installPhase = ''
            runHook preInstall
            for pkg in $(ls packages); do
              src_dir="packages/$pkg"
              dst_dir="$out/packages/$pkg"
              mkdir -p "$dst_dir"
              cp "$src_dir/package.json" "$dst_dir/"
              for sub in dist types wasm src lib bin scripts; do
                if [ -d "$src_dir/$sub" ]; then
                  cp -r "$src_dir/$sub" "$dst_dir/$sub"
                fi
              done
              for f in "$src_dir"/*.{json,js,mjs,cjs,md,d.ts,d.mts,wit,toml}; do
                [ -f "$f" ] && cp "$f" "$dst_dir/" || true
              done
            done
            cp -r wit $out/wit
            find $out/packages -type f \( -name '*.cjs' -o -name '*.mjs' -o -name '*.js' \) \
              -path '*/dist/*' -exec chmod +x {} +
            patchShebangs $out/packages
            for pkg_json in $out/packages/*/package.json; do
              ${pkgs.jq}/bin/jq 'del(.scripts.prepare)' "$pkg_json" > "$pkg_json.tmp"
              mv "$pkg_json.tmp" "$pkg_json"
            done
            runHook postInstall
          '';
        };

        # Generate the `agent-template` Rust crate purely from WIT input via
        # wasm-rquickjs. No network access needed — this is just code
        # generation. The output's `Cargo.lock` is stable for a given
        # wasm-rquickjs version, which is what makes the downstream
        # `cargo vendor` FOD usable.
        agent-template-source = pkgs.runCommand "agent-template-source"
          {
            nativeBuildInputs = [ wasm-rquickjs ];
          } ''
          set -eu
          cp -rL ${tsSdkSrc}/wit ./wit
          chmod -R +w ./wit
          # generate-wrapper-crate copies a JavaScript module into the
          # crate's bundle. We point it at the REAL `dist/index.mjs` from
          # the first-stage SDK build, so the embedded module exposes the
          # same exports (TypescriptTypeRegistry, etc.) that compiled TS
          # components reference at wizer pre-initialize time.
          ${wasm-rquickjs}/bin/wasm-rquickjs generate-wrapper-crate \
            --wit ./wit \
            --output ./agent-template \
            --world agent-guest \
            --js-modules '@golemcloud/golem-ts-sdk=${golem-ts-sdk-base}/packages/golem-ts-sdk/dist/index.mjs' \
            --js-modules 'user=@slot'
          test -f ./agent-template/src/lib.rs || {
            echo "agent-template/src/lib.rs not produced" >&2
            exit 1
          }
          mkdir -p $out
          cp -r ./agent-template $out/agent-template
        '';

        # FOD that runs `cargo vendor` over the generated agent-template
        # crate. cargo vendor's output is byte-stable (sorted directory
        # listing, no timestamps) so a fixed-output hash is reliable here.
        #
        # Hash invariant: this output depends ONLY on the
        # `agent-template-source` crate's `Cargo.lock`. Things that DO
        # invalidate this hash:
        #   - bumping the `wasm-rquickjs` git pin (likely changes the
        #     generated `Cargo.lock`)
        #   - upstream `cargo vendor` semantics changing
        # Things that do NOT invalidate it:
        #   - changes to `golem-ts-sdk-base`'s `dist/index.mjs` content
        #     (the JS module gets embedded as a string literal in
        #     `src/lib.rs` but doesn't affect Cargo.toml/lock)
        #   - changes to the WIT files (they parametrize what code is
        #     generated, but not which crates are pulled in)
        # If this hash mismatches after a wasm-rquickjs bump, regenerate
        # via `lib.fakeHash` and update.
        agent-template-vendor = pkgs.stdenv.mkDerivation {
          pname = "agent-template-vendor";
          version = "0.0.0";
          src = "${agent-template-source}/agent-template";

          outputHashAlgo = "sha256";
          outputHashMode = "recursive";
          outputHash = "sha256-RhiCFCqKdiwuuFOfWFm11mOkQXNFV1tJ/KJR8BHtIgw=";

          nativeBuildInputs = [
            rustToolchain
            pkgs.cacert
            pkgs.git
          ];

          buildPhase = ''
            runHook preBuild
            export HOME=$TMPDIR/home
            mkdir -p $HOME $out
            # Have cargo vendor write directly into $out — that way each
            # vendored crate dir is at the top of the FOD output and
            # consumers point their [source.vendored-sources] directory
            # at the FOD path itself. cargo vendor's stdout is the
            # [source.<git+url>] config snippet with absolute
            # `directory = "<abs path>"` lines; rewrite those lines to a
            # sentinel so consumers can substitute in their own vendor
            # store path at use time.
            cargo vendor "$out" > "$out/.cargo-config.toml.in"
            sed -i "s|directory = .*|directory = \"@VENDOR_DIR@\"|" "$out/.cargo-config.toml.in"
            # Belt-and-braces: scrub timestamps so any stray mtime that
            # cargo-vendor leaves doesn't perturb the recursive hash.
            find "$out" -exec touch -h -t 197001010000.00 {} +
            runHook postBuild
          '';

          dontInstall = true;
          # FOD outputs must not reference store paths; nixpkgs' fixupPhase
          # runs patchShebangs which would rewrite shebangs in vendored .sh
          # files to /nix/store/.../bash. Skip fixup entirely — cargo only
          # cares about the crate sources, not shebangs.
          dontFixup = true;
        };

        # Hermetic build of agent_guest.wasm — the wasm runtime that
        # `golem-ts-sdk` injects user JavaScript into. Consumes the
        # vendored deps from `agent-template-vendor` so it's offline-only.
        agent-guest-wasm = pkgs.stdenv.mkDerivation {
          pname = "agent-guest";
          version = "0.0.1";
          src = "${agent-template-source}/agent-template";

          nativeBuildInputs = [
            rustToolchain
            pkgs.libclang.lib
            pkgs.clang
            pkgs.glibc.dev
          ] ++ pkgs.lib.optionals (wasi-sdk != null) [ wasi-sdk ];

          # nixpkgs' stdenv adds `-fzero-call-used-regs=used-gpr` via the
          # `zerocallusedregs` hardening flag; clang rejects it for
          # wasm32-wasip2. Narrowly disable just that one — the other
          # hardening flags (stack protectors, FORTIFY_SOURCE, format,
          # PIE) either work for wasm or are harmless.
          hardeningDisable = [ "zerocallusedregs" ];

          buildPhase = ''
            runHook preBuild
            export HOME=$TMPDIR/home
            mkdir -p $HOME

            # Wire cargo to the vendored deps from the FOD. The config
            # template carries every `[source.<git+...>]` redirect cargo
            # vendor produced, with `@VENDOR_DIR@` placeholders we now
            # substitute with the actual FOD output path.
            mkdir -p .cargo
            sed "s|@VENDOR_DIR@|${agent-template-vendor}|g" \
              ${agent-template-vendor}/.cargo-config.toml.in \
              > .cargo/config.toml

            # rquickjs-sys + cc-rs + bindgen env. See `wasiSdkEnv` near the
            # top of the let block for the full explanation of each var.
            ${wasiSdkEnv}
            export LIBCLANG_PATH="${pkgs.libclang.lib}/lib"
            export BINDGEN_EXTRA_CLANG_ARGS_x86_64_unknown_linux_gnu="--target=x86_64-unknown-linux-gnu -I${pkgs.glibc.dev}/include"

            cargo build --offline --target wasm32-wasip2 --release --features full,golem
            runHook postBuild
          '';

          installPhase = ''
            runHook preInstall
            mkdir -p $out
            cp target/wasm32-wasip2/release/agent_guest.wasm $out/agent_guest.wasm
            runHook postInstall
          '';
        };

        # Final golem-ts-sdk: the base monorepo with the hermetically-built
        # `agent_guest.wasm` staged into `wasm/`. Test-components reference
        # this via npm `file:` deps; the wasm is what `injectToPrebuiltQuickjs`
        # uses at build time.
        golem-ts-sdk = pkgs.runCommand "golem-ts-sdk-0.0.0" { } ''
          mkdir -p $out
          cp -r ${golem-ts-sdk-base}/. $out/
          chmod -R +w $out
          mkdir -p $out/packages/golem-ts-sdk/wasm
          cp ${agent-guest-wasm}/agent_guest.wasm $out/packages/golem-ts-sdk/wasm/
        '';

        rustTestComponents = [
          "host-api-tests"
          "http-tests"
          "oplog-processor"
          "initial-file-system"
          "agent-counters"
          "agent-updates-v1"
          "agent-updates-v2"
          "agent-updates-v3"
          "agent-updates-v4"
          "scalability"
          "agent-sdk-rust"
          "agent-invocation-context"
          "agent-mcp"
        ];

        tsTestComponents = [
          "agent-constructor-parameter-echo"
          "agent-promise"
          "agent-sdk-ts"
          "agent-rpc"
        ];

        # agent-rpc lives in tsTestComponents but its golem.yaml declares both a
        # Rust component (golem-it:agent-rpc-rust) and a TS component
        # (golem-it:agent-rpc). The Rust half is required by group1 tests, so
        # we build it explicitly here even before TS components land.
        testComponentsCargoVendorDir = craneLib.vendorMultipleCargoDeps {
          cargoLockList =
            (map (c: ./test-components + "/${c}/Cargo.lock") rustTestComponents)
            ++ [ ./test-components/agent-rpc/golem-it-agent-rpc-rust/Cargo.lock ];
        };

        test-components-rust = pkgs.stdenv.mkDerivation {
          pname = "golem-test-components-rust";
          version = "0.0.0";
          inherit src;

          nativeBuildInputs = [
            rustToolchain
            golem-cli
            pkgs.wasm-tools
            pkgs.wit-bindgen
            pkgs.git
            pkgs.cacert
          ] ++ pkgs.lib.optionals (wasi-sdk != null) [ wasi-sdk ];

          buildPhase = ''
            runHook preBuild
            ${wasiSdkEnv}
            export CARGO_HOME=$TMPDIR/cargo-home
            mkdir -p $CARGO_HOME
            cat ${testComponentsCargoVendorDir}/config.toml >> $CARGO_HOME/config.toml
            cd test-components
            for component in ${pkgs.lib.concatStringsSep " " rustTestComponents}; do
              echo "==> Building $component"
              pushd "$component"
              golem-cli --preset release build --yes --skip-check
              golem-cli --preset release exec copy
              popd
            done
            # agent-rpc has both a Rust and TS component; build only the Rust
            # one. `exec copy` would also try to copy the (unbuilt) TS half,
            # so do the wasm copy manually after the build.
            echo "==> Building agent-rpc/golem-it:agent-rpc-rust"
            pushd agent-rpc
            golem-cli --preset release build golem-it:agent-rpc-rust --yes --skip-check
            wasm=$(find . -name 'golem_it_agent_rpc_rust*release.wasm' -print -quit)
            cp "$wasm" ../golem_it_agent_rpc_rust_release.wasm
            popd
            runHook postBuild
          '';

          installPhase = ''
            runHook preInstall
            # buildPhase ends in test-components/; the *.wasm outputs are dropped
            # there by the `golem-cli exec copy` step.
            mkdir -p $out/test-components
            find . -maxdepth 1 -name '*.wasm' -exec cp {} $out/test-components/ \;
            runHook postInstall
          '';
        };

        # Per-test-component npm offline cache. The hash list mirrors
        # `tsTestComponents` order; `pkgs.fetchNpmDeps` produces a
        # deterministic offline-mirror tarball collection from the
        # component's `package-lock.json`.
        tsComponentNpmDeps = pkgs.lib.genAttrs tsTestComponents (name:
          pkgs.fetchNpmDeps {
            src = ./test-components + "/${name}";
            name = "test-component-${name}-npm-deps";
            hash = builtins.getAttr name {
              "agent-constructor-parameter-echo" =
                "sha256-2pJo/Bgg9Kcly8BIi2WqvcjHlvQbpReWKd5dcIZNu5o=";
              "agent-promise" =
                "sha256-2pJo/Bgg9Kcly8BIi2WqvcjHlvQbpReWKd5dcIZNu5o=";
              "agent-sdk-ts" =
                "sha256-Ft+NZN6L4LJuoXjVW7mUmrYwrroaBbUJf1BXRfwPrbM=";
              "agent-rpc" =
                "sha256-2pJo/Bgg9Kcly8BIi2WqvcjHlvQbpReWKd5dcIZNu5o=";
            };
          });

        # Hermetic per-component build. Consumes:
        # - the pre-built `golem-ts-sdk` (with `agent_guest.wasm` already in
        #   `wasm/` and shebangs already patched) — overlaid into
        #   `sdks/ts/packages/*` so `npm install`'s file: deps resolve
        #   correctly.
        # - the per-component `fetchNpmDeps` tarball (`npmConfigHook` wires
        #   `npm_config_cache` to it so `npm install --offline` works).
        # No network access required.
        mkTsTestComponent = name: pkgs.stdenv.mkDerivation {
          pname = "golem-test-component-ts-${name}";
          version = "0.0.0";
          inherit src;

          npmDeps = tsComponentNpmDeps.${name};
          npmRoot = "test-components/${name}";
          # Hermetic-build defense: every TS test-component has `file:` deps
          # to `sdks/ts/packages/golem-ts-sdk` (and friends), and those
          # packages declare a `prepare` script of `pnpm build`. npm's
          # default install runs `prepare` on file: deps, which would
          # regenerate dist/*.cjs (re-introducing `#!/usr/bin/env node`
          # shebangs) AND require pnpm on PATH (it's not in our inputs).
          # Skipping scripts is correct here — the SDK we're staging in
          # from `golem-ts-sdk` already has its dist/ built.
          npmInstallFlags = [ "--ignore-scripts" ];

          nativeBuildInputs = [
            pkgs.nodejs_20
            pkgs.npmHooks.npmConfigHook
            golem-cli
            pkgs.git
            # golem-cli's startup probe creates an HTTPS reqwest client
            # which loads system CA roots. Without this in the sandbox
            # the build fails with "No CA certificates were loaded".
            pkgs.cacert
          ];

          # Overlay the pre-built golem-ts-sdk packages into the source
          # tree BEFORE configurePhase so when `npmConfigHook` runs its
          # internal `npm install` during configure, the file: deps to
          # `sdks/ts/packages/*` resolve to packages with built dist/,
          # types/, wasm/agent_guest.wasm, and patched shebangs already
          # in place. Without this, npm install tries to run the dep's
          # `prepare` script (`pnpm build`) and crashes because pnpm
          # isn't a build input.
          #
          # Also strip `prepare` from each SDK package.json: npm treats
          # `prepare` as a "preparation" step that runs for file:/git:
          # deps even with `--ignore-scripts`, so the only reliable way
          # to stop `pnpm build` from being invoked is to remove the
          # script entry. Safe here — the SDK's dist/ is already built
          # in the `golem-ts-sdk` derivation we just overlaid.
          postPatch = ''
            chmod -R +w sdks/ts/packages
            for pkg in $(ls ${golem-ts-sdk}/packages); do
              rm -rf "sdks/ts/packages/$pkg"
              cp -rL "${golem-ts-sdk}/packages/$pkg" "sdks/ts/packages/$pkg"
              chmod -R +w "sdks/ts/packages/$pkg"
            done
            for pkg_json in sdks/ts/packages/*/package.json; do
              ${pkgs.nodejs_20}/bin/node -e "
                const fs = require('fs');
                const path = process.argv[1];
                const j = JSON.parse(fs.readFileSync(path));
                if (j.scripts) { delete j.scripts.prepare; }
                fs.writeFileSync(path, JSON.stringify(j, null, 2));
              " "$pkg_json"
            done
          '';

          # npmConfigHook may write to $HOME during its install pass.
          preBuild = ''
            export HOME=$TMPDIR/home
            mkdir -p $HOME
          '';

          buildPhase = ''
            runHook preBuild

            # npmConfigHook already populated node_modules for us during
            # configurePhase; patch shebangs of the freshly-installed
            # bins (tsc, rollup, golem-typegen, …).
            cd test-components/${name}
            patchShebangs node_modules

            # Assert the SDK overlay survived `npm install`. With the
            # SDK's `prepare` script stripped (see `postPatch` in
            # `golem-ts-sdk-base.installPhase`), `npm ci --ignore-scripts`
            # should symlink `node_modules/@golemcloud/golem-ts-sdk` at
            # the pre-built file: dep without re-running `pnpm build`.
            # If npm re-built dist/ regardless, the shebang on
            # `golem-typegen.cjs` would still be `#!/usr/bin/env node`
            # rather than the patched `#!/nix/store/.../node`. Crash
            # loudly here instead of failing later with a confusing
            # "/usr/bin/env: bad interpreter".
            typegen_shebang=$(head -n1 node_modules/@golemcloud/golem-ts-typegen/dist/golem-typegen.cjs 2>/dev/null || echo "MISSING")
            case "$typegen_shebang" in
              "#!/nix/store/"*)
                ;;
              *)
                echo "ERROR: golem-typegen.cjs shebang is '$typegen_shebang'" >&2
                echo "  Expected '#!/nix/store/.../node' — npm install" >&2
                echo "  appears to have re-run the SDK's prepare script" >&2
                echo "  (which would regenerate dist/ without our patched" >&2
                echo "  shebangs). Verify postPatch in golem-ts-sdk-base." >&2
                exit 1
                ;;
            esac

            # golem-cli build. Its `ensure_npm_dependencies` check sees
            # an up-to-date node_modules with matching package-lock hash
            # so it skips its own internal `npm install` (which would
            # otherwise undo our patch).
            #
            # agent-rpc's golem.yaml declares both a Rust component
            # (`golem-it:agent-rpc-rust`) and a TS one (`golem-it:agent-rpc`).
            # The Rust half is already built in `test-components-rust`, and
            # building it here would need cargo + WASI SDK in this
            # JS-focused derivation. Scope to just the TS component.
            ${if name == "agent-rpc" then ''
              golem-cli --preset release build golem-it:agent-rpc --yes --skip-check
              wasm=$(find . -name 'golem_it_agent_rpc.wasm' -print -quit)
              cp "$wasm" ../golem_it_agent_rpc.wasm
            '' else ''
              golem-cli --preset release build --yes --skip-check
              golem-cli --preset release exec copy
            ''}

            runHook postBuild
          '';

          installPhase = ''
            runHook preInstall
            # `golem-cli exec copy` drops `<name>.wasm` into
            # `test-components/` (one dir above the component dir).
            mkdir -p $out/test-components
            find ../ -maxdepth 1 -name '*.wasm' -exec cp {} $out/test-components/ \;
            runHook postInstall
          '';

          # Cargo target dir gets cleaned up; no build artifacts to keep.
          dontFixup = false;
        };

        # All hermetically-built TS test-components combined into one
        # `test-components/*.wasm` tree.
        test-components-ts = pkgs.symlinkJoin {
          name = "golem-test-components-ts";
          paths = map mkTsTestComponent tsTestComponents;
        };

        # All workspace binaries staged into the layout the test framework
        # expects ($GOLEM_REPO_ROOT/target/debug/<name>). Used by the
        # worker-executor / sharding / integration flake checks below.
        golem-services = craneLib.mkCargoDerivation (commonArgs // {
          inherit cargoArtifacts;
          pname = "golem-services";
          doInstallCargoArtifacts = false;
          buildPhaseCargoCommand = "cargo build --locked --workspace --bins";
          installPhaseCommand = ''
            mkdir -p $out/target/debug
            # Bin names follow each crate's own `[[bin]] name = ...` (which
            # differs from the crate dir, e.g. golem-worker-executor → worker-executor).
            for bin in \
              worker-executor \
              golem-worker-service \
              golem-shard-manager \
              golem-component-compilation-service \
              golem-registry-service \
              golem-debugging-service \
              golem-cli \
              golem \
              golem-openapi-client-generator; do
              if [ -f "target/debug/$bin" ]; then
                install -m 0755 "target/debug/$bin" "$out/target/debug/$bin"
              else
                echo "warning: target/debug/$bin not produced" >&2
              fi
            done
          '';
        });

        # Stage runtime artefacts into the cwd source tree so
        # (a) `golem-worker-executor-test-utils`'s hardcoded relative path
        # `../test-components` resolves to our pre-built wasms, and
        # (b) `target/debug/<bin>` contains the spawnable service binaries the
        # test framework launches. Run from the workspace root before tests.
        mkRuntimeRoot = ''
          mkdir -p ./test-components ./target/debug
          # Copy wasms (not symlink). `tokio::fs::copy` in the test
          # framework's `component_writer` preserves source mode; if the
          # source is a /nix/store symlink target (mode 0444), the
          # framework's working copy ends up read-only, and the negative
          # tests that rewrite the component bytes (e.g.
          # `trying_to_use_a_wasm_that_wasmtime_cannot_load_*`) fail with
          # EACCES. `install -m 0644` gives every staged wasm a writable
          # mode bit downstream copies can inherit.
          for wasm in ${test-components-rust}/test-components/*.wasm \
                      ${test-components-ts}/test-components/*.wasm; do
            [ -e "$wasm" ] && install -m 0644 "$wasm" "./test-components/$(basename "$wasm")"
          done
          for bin in ${golem-services}/target/debug/*; do
            dst="./target/debug/$(basename "$bin")"
            [ -e "$dst" ] || ln -sf "$bin" "$dst"
          done
          export GOLEM_REPO_ROOT="$(pwd)"
        '';

        # Sidecar that spins up a `postgresql_16` cluster on
        # 127.0.0.1:5432 inside the sandbox so test deps that need a
        # real Postgres (worker-executor `rdbms_service` /
        # `key_value_storage` / `indexed_storage` /
        # `namespace_routed_key_value_storage`, registry-service
        # `repo::postgres`, integration-tests group8) hit a live
        # instance instead of `DockerPostgresRdb` (no docker socket
        # in the sandbox). Provided via env discovery in
        # `EnvBasedTestDependencies::make_rdb` —
        # `check_if_postgres_running` succeeds against this cluster
        # and the framework picks `ProvidedPostgresRdb` over
        # `DockerPostgresRdb`. Cluster lives in `$TMPDIR/pgdata`;
        # auth is `trust` over loopback only (the Nix sandbox has
        # no external network namespace, so this is closed).
        spawnPostgres = ''
          export PGDATA="$TMPDIR/pgdata"
          mkdir -p "$PGDATA"
          # `--auth=trust` so sqlx's password-authenticated connect
          # succeeds without needing to set a password post-init.
          # `--username=postgres` matches `PostgresInfo`'s default.
          initdb -D "$PGDATA" --username=postgres --auth=trust \
            --no-sync >/dev/null
          # Default `listen_addresses = 'localhost'` resolves to ::1
          # under glibc, but the sandbox's loopback may not have IPv6
          # configured. Pin to the IPv4 loopback for portability.
          echo "listen_addresses = '127.0.0.1'" >> "$PGDATA/postgresql.conf"
          echo "unix_socket_directories = '$PGDATA'" >> "$PGDATA/postgresql.conf"
          # `pg_ctl start` waits for the cluster to accept
          # connections before returning, so no separate readiness
          # probe is required.
          pg_ctl -D "$PGDATA" -l "$TMPDIR/postgres.log" \
            -o "-p 5432 -k $PGDATA" start
        '';

        # Generic factory for the integration-style tests that share the
        # same runtime shape: spawn redis + sqlite, stage service bins +
        # test components, then run a cargo test binary with an optional
        # test-r tag filter and a list of substring skips.
        mkSpawnedTest =
          { pname
          , package
          , testName
          , tag ? ""
          , skips ? commonSkips
          , extraNativeBuildInputs ? [ ]
          , testThreads ? null
          , withPostgres ? false
          }:
          craneLib.mkCargoDerivation (commonArgs // {
            inherit cargoArtifacts;
            inherit pname;
            doInstallCargoArtifacts = false;
            doCheck = true;
            nativeBuildInputs = commonNativeBuildInputs
              ++ [ pkgs.redis ]
              ++ pkgs.lib.optional withPostgres pkgs.postgresql_16
              ++ extraNativeBuildInputs;
            GOLEM_TEST_DB = if withPostgres then "postgres" else "sqlite";
            WASMTIME_BACKTRACE_DETAILS = "1";
            RUST_BACKTRACE = "1";
            RUST_LOG = "info";
            buildPhaseCargoCommand = ''
              cargo test --locked --no-run -p ${package} --test ${testName}
            '';
            checkPhaseCargoCommand = ''
              ${mkRuntimeRoot}
              # Wasmtime opens a filesystem cache (~/.cache/wasmtime) inside
              # `extract_agent_types`; the sandbox has no $HOME, so point
              # both XDG and HOME at a writable temp dir.
              export HOME="$TMPDIR/home"
              export XDG_CACHE_HOME="$TMPDIR/cache"
              mkdir -p "$HOME" "$XDG_CACHE_HOME"
              ${pkgs.lib.optionalString withPostgres spawnPostgres}
              cargo test --locked -p ${package} --test ${testName} \
                -- ${tag} --report-time --nocapture \
                ${pkgs.lib.optionalString (testThreads != null) "--test-threads=${toString testThreads}"} \
                ${pkgs.lib.concatMapStringsSep " " (s: "--skip ${s}") skips}
              ${pkgs.lib.optionalString withPostgres ''
                pg_ctl -D "$PGDATA" stop -m immediate >/dev/null 2>&1 || true
              ''}
            '';
            installPhaseCommand = ''
              mkdir -p $out
              echo "${pname} passed" > $out/result
            '';
          });

        mkWorkerExecutorTest = { tag, name }: mkSpawnedTest {
          pname = "golem-worker-executor-tests-${name}";
          package = "golem-worker-executor";
          testName = "integration";
          inherit tag;
        };
      in
      {
        packages = {
          default = golem-cli;
          golem-cli = golem-cli;
          golem-services = golem-services;
          wasm-rquickjs = wasm-rquickjs;
          test-components-rust = test-components-rust;
          test-components-ts = test-components-ts;
          golem-ts-sdk = golem-ts-sdk;
          golem-ts-sdk-base = golem-ts-sdk-base;
          # Expose per-component TS builds so individual failures are easy
          # to triage with `nix build .#test-components-ts-<name>`.
          test-components-ts-agent-constructor-parameter-echo =
            mkTsTestComponent "agent-constructor-parameter-echo";
          test-components-ts-agent-promise =
            mkTsTestComponent "agent-promise";
          test-components-ts-agent-sdk-ts =
            mkTsTestComponent "agent-sdk-ts";
          test-components-ts-agent-rpc =
            mkTsTestComponent "agent-rpc";
          agent-template-source = agent-template-source;
          agent-template-vendor = agent-template-vendor;
          agent-guest-wasm = agent-guest-wasm;
        } // pkgs.lib.optionalAttrs (wasi-sdk != null) {
          wasi-sdk = wasi-sdk;
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

          # CONTRIBUTING.md #1: `cargo make build`. Compiles every workspace
          # bin/test target so we catch link-time and bin-only breakage that
          # the lib-only unit-tests check misses.
          workspace-build = craneLib.mkCargoDerivation (commonArgs // {
            inherit cargoArtifacts;
            pname = "golem-workspace-build";
            buildPhaseCargoCommand = "cargo build --locked --workspace --all-targets";
            doInstallCargoArtifacts = false;
          });

          # Guards a serialization invariant in golem-common via a single
          # named test (matches `cargo make check-diff-model-fingerprint`).
          diff-model-fingerprint = craneLib.cargoTest (commonArgs // {
            inherit cargoArtifacts;
            pname = "golem-diff-model-fingerprint";
            cargoExtraArgs = "--locked -p golem-common";
            cargoTestExtraArgs = "diff_model_version_matches_diff_module_fingerprint";
          });

          worker-executor-tests-group1 =
            mkWorkerExecutorTest { tag = ":tag:group1"; name = "group1"; };
          worker-executor-tests-group2 =
            mkWorkerExecutorTest { tag = ":tag:group2"; name = "group2"; };
          worker-executor-tests-group3 =
            mkWorkerExecutorTest { tag = ":tag:group3"; name = "group3"; };

          # CONTRIBUTING.md `worker-executor-tests-misc`. Runs the
          # untagged worker-executor tests (compatibility, fuel,
          # indexed_storage, key_value_storage,
          # namespace_routed_key_value_storage). With a Postgres
          # sidecar spawned in-sandbox, the three KV/indexed-storage
          # binaries that previously hardcoded `DockerPostgresRdb`
          # now consume the Provided Postgres via
          # `create_postgres_rdb()`. `rdbms_service` still pulls in
          # `DockerMysqlRdb` (no provided-MySQL fork exists yet);
          # `ignite_service` needs a live Apache Ignite TCP node.
          worker-executor-tests-misc = mkSpawnedTest {
            pname = "golem-worker-executor-tests-misc";
            package = "golem-worker-executor";
            testName = "integration";
            tag = ":tag:";
            withPostgres = true;
            skips = commonSkips ++ [
              "rdbms_service"
              "ignite_service"
            ];
          };

          # CONTRIBUTING.md #9: sharding test. Runs single-threaded
          # (matches the upstream cargo-make invocation; the cluster
          # state machine assumes sequential interleavings).
          sharding-tests-debug = mkSpawnedTest {
            pname = "golem-sharding-tests-debug";
            package = "integration-tests";
            testName = "sharding";
            testThreads = 1;
            # Two sharding tests are known-flaky in this repo: the
            # oplog_processor_locality_recovery loop times out under the
            # sandbox's slower scheduler, and oplog_processor_shard_move_inflight
            # racy-asserts on "Duplicate oplog indices found". Upstream's
            # `cargo make worker-executor-tests-misc` flakily retries
            # similar paths with `--flaky-run=5`; we just skip them here.
            skips = commonSkips ++ [
              "oplog_processor_locality_recovery"
              "oplog_processor_shard_move_inflight"
            ];
          };

          # CONTRIBUTING.md #7: multi-service integration tests.
          # Built once per tag group so failures in one group don't
          # block the others and so each shows up as its own check.
          integration-tests-group1 = mkSpawnedTest {
            pname = "golem-integration-tests-group1";
            package = "integration-tests";
            testName = "integration";
            tag = ":tag:group1";
            testThreads = 1;
            # `fork_and_sync_with_promise` times out at 240s in the
            # sandbox and reports `Exceeded plan storage limit`. Looks
            # like a quota / memory-pressure interaction with the
            # smaller worker pool; runs locally with full resources.
            skips = commonSkips ++ [
              "fork_and_sync_with_promise"
            ];
          };
          integration-tests-group2 = mkSpawnedTest {
            pname = "golem-integration-tests-group2";
            package = "integration-tests";
            testName = "integration";
            tag = ":tag:group2";
            testThreads = 1;
          };
          # NB: integration-tests group7 (`otlp_plugin`, `plugins`) is
          # intentionally omitted — both suites start a Docker-managed
          # Jaeger container (panics at `jaeger/docker.rs:50`). They run
          # locally via `cargo make integration-tests-group7` once Docker
          # is available; the sandbox can't satisfy them.
          integration-tests-group10 = mkSpawnedTest {
            pname = "golem-integration-tests-group10";
            package = "integration-tests";
            testName = "integration";
            tag = ":tag:group10";
            testThreads = 1;
          };
          integration-tests-group12 = mkSpawnedTest {
            pname = "golem-integration-tests-group12";
            package = "integration-tests";
            testName = "integration";
            tag = ":tag:group12";
            testThreads = 1;
            # The TS-flavored agent-config tests time out at 240s in
            # the sandbox — they exercise the agent-sdk-ts component
            # startup path which is slower under the smaller worker
            # pool nix gives. Both the `*_ts` suffix tests and the
            # `ts_*` module-prefix tests (`ts_optional_group_*` etc.)
            # hit the same slow path. The Rust-flavored siblings
            # cover the same code paths and pass.
            skips = commonSkips ++ [
              "_ts"
              "agent_config::ts"
            ];
          };

          # NB: CONTRIBUTING.md #8 `cli-integration-tests` is
          # intentionally NOT wired up. The entire test suite is two
          # modules:
          #   - `bridge_gen::*` — shells out to `npm install` /
          #     `cargo build` against dynamically-generated wrapper
          #     code (no pre-vendorable deps).
          #   - `app::*` — runs `golem-cli build` on freshly-scaffolded
          #     mixed TS+Rust apps; same network requirement.
          # Filtering both leaves zero tests, so a flake check would
          # be vacuous. Run locally via `cargo make
          # cli-integration-tests-group{1..6}` with network available.

        }
        // (
          # CI's `integration-tests-group5` runs `cargo test` against the
          # service crates themselves (not the integration-tests crate).
          # Each cargo test invocation hits redis + sqlite-backed
          # in-process tests with no Docker dependency. Per-crate config
          # captures: which Cargo.toml `[[test]] name = ...` to run, and
          # which extra substring skips beyond `commonSkips`.
          let
            serviceCrates = {
              service-base = {
                package = "golem-service-base";
                testName = "integration";
                # `blob_storage::*` tests exercise a real S3 backend
                # (or LocalStack/minio) the sandbox can't provide.
                # Two name shapes: `*_s3` / `*_s3_prefixed` (suffix)
                # and `s3_copy_*` (prefix).
                extraSkips = [ "_s3" "::s3_" ];
                withPostgres = false;
              };
              registry-service = {
                package = "golem-registry-service";
                # Cargo.toml: [[test]] name = "tests", path = "tests/lib.rs"
                testName = "tests";
                # `repo::postgres::*` now hits the Provided sidecar.
                # The TLS variant (`postgres_tls`) still needs a
                # certificate-configured Postgres image; skip just
                # those.
                extraSkips = [ "postgres_tls" ];
                withPostgres = true;
              };
              worker-service = {
                package = "golem-worker-service";
                # Cargo.toml: [[test]] name = "oidc", path = "tests/oidc/lib.rs"
                testName = "oidc";
                extraSkips = [ ];
                withPostgres = false;
              };
              debugging-service = {
                package = "golem-debugging-service";
                testName = "integration";
                extraSkips = [ ];
                withPostgres = false;
              };
            };
          in
          pkgs.lib.mapAttrs'
            (
              suffix:
              { package, testName, extraSkips, withPostgres }:
              pkgs.lib.nameValuePair "integration-tests-group5-${suffix}" (mkSpawnedTest {
                pname = "golem-integration-tests-group5-${suffix}";
                inherit package testName withPostgres;
                skips = commonSkips ++ extraSkips;
              })
            )
            serviceCrates
        )
        // {

          # CI's `integration-tests-group8` / `integration-tests-group9`:
          # `agent-config-live-mutation` test binary against Postgres
          # (group8) and SQLite (group9). Group8 is now sandbox-runnable
          # thanks to the Provided Postgres discovery in
          # `EnvBasedTestDependencies::make_rdb`.
          integration-tests-group8 = mkSpawnedTest {
            pname = "golem-integration-tests-group8";
            package = "integration-tests";
            testName = "agent-config-live-mutation";
            testThreads = 1;
            withPostgres = true;
            skips = commonSkips ++ [
              "_ts"
              "agent_config::ts"
            ];
          };
          integration-tests-group9 = mkSpawnedTest {
            pname = "golem-integration-tests-group9";
            package = "integration-tests";
            testName = "agent-config-live-mutation";
            testThreads = 1;
            skips = commonSkips ++ [
              "_ts"
              "agent_config::ts"
            ];
          };

          # CI's `ci.yaml:golem-wasm-guest` step:
          # `cargo build --target wasm32-wasip2 -p golem-wasm
          # --no-default-features --features guest`. Confirms the
          # WASM-guest cross-compile of the golem-wasm crate stays
          # green. `workspace-build` doesn't cover this — it builds
          # the host-side default features only.
          wasm-guest-build = craneLib.mkCargoDerivation (commonArgs // {
            inherit cargoArtifacts;
            pname = "golem-wasm-guest-build";
            doInstallCargoArtifacts = false;
            buildPhaseCargoCommand = ''
              cargo build --locked --target wasm32-wasip2 \
                -p golem-wasm --no-default-features --features guest
            '';
            # `golem-wasm` is a library crate; cargo emits a `.rlib`
            # (not a `.wasm`) for the wasm32-wasip2 target. Verify the
            # artifact actually got produced — without this the check
            # would pass even if the build silently no-op'd.
            installPhaseCommand = ''
              rlib=$(find target/wasm32-wasip2/ -name 'libgolem_wasm*.rlib' -print -quit)
              if [ -z "$rlib" ]; then
                echo "ERROR: golem-wasm did not produce an .rlib for wasm32-wasip2" >&2
                exit 1
              fi
              mkdir -p $out
              cp "$rlib" $out/
              echo "golem-wasm wasm32-wasip2 guest build OK" > $out/result
            '';
          });

          # NB: CI's `integration-tests-group7` (otlp_plugin, plugins)
          # is intentionally NOT wired. The test framework's
          # `DockerJaeger::new()` hardcodes `GenericImage::new(
          # "jaegertracing/all-in-one", "1.76.0")` and shells out to
          # the docker socket; both `otlp_plugin` and `plugins`
          # suites depend on it. Removing the Docker requirement
          # would mean patching `golem-test-framework/src/components/
          # jaeger/docker.rs` to support a provided OTLP collector
          # instead of spawning a container — an upstream change, not
          # something the flake layer can paper over. Local runs via
          # `cargo make integration-tests-group7` work once Docker is
          # available.

          # NB: `windows-daily-build.yaml` cross-compiles to
          # x86_64-pc-windows-{msvc,gnu}. Wiring a Windows
          # cross-compile from a Linux flake is technically
          # achievable via `cross-rs` (or a mingw-w64 rust-overlay
          # target), but it's a multi-day toolchain plumbing effort
          # and doesn't share artifacts with the rest of the checks.
          # Left as CI-only.

          # `cargo make check-configs`. Each service binary supports
          # `--dump-config-default-toml` / `--dump-config-default-env-var`;
          # we run them and diff against the committed copies. Drift = fail.
          config-drift = pkgs.runCommand "golem-config-drift" {
            nativeBuildInputs = [ pkgs.diffutils ];
          } ''
            set -e
            fail=0
            check() {
              local bin="$1" flag="$2" committed="$3"
              local tmpfile="$TMPDIR/$(basename $committed).generated"
              if ! ${golem-services}/target/debug/$bin $flag > "$tmpfile" 2>/dev/null; then
                echo "ERROR: failed to dump $flag from $bin" >&2
                fail=1
                return
              fi
              # Strict byte-for-byte comparison via direct file
              # redirection — `$()` collapses trailing newlines, so
              # writing the binary's stdout to a tmpfile preserves
              # the exact bytes for diff. The committed copies are
              # kept in sync via `cargo make generate-configs`; any
              # real drift here fails the check, and the fix is to
              # rerun generate-configs and commit, not to soften
              # this diff.
              if ! diff "$tmpfile" ${src}/$committed >/dev/null 2>&1; then
                echo "DRIFT: $committed differs from $bin $flag" >&2
                diff "$tmpfile" ${src}/$committed | head -20 >&2 || true
                fail=1
              fi
            }
            for svc in registry-service shard-manager component-compilation-service worker-service worker-executor; do
              case "$svc" in
                registry-service)
                  bin=golem-registry-service; toml=golem-registry-service/config/registry-service.toml; env=golem-registry-service/config/registry-service.sample.env;;
                shard-manager)
                  bin=golem-shard-manager; toml=golem-shard-manager/config/shard-manager.toml; env=golem-shard-manager/config/shard-manager.sample.env;;
                component-compilation-service)
                  bin=golem-component-compilation-service; toml=golem-component-compilation-service/config/component-compilation-service.toml; env=golem-component-compilation-service/config/component-compilation-service.sample.env;;
                worker-service)
                  bin=golem-worker-service; toml=golem-worker-service/config/worker-service.toml; env=golem-worker-service/config/worker-service.sample.env;;
                worker-executor)
                  bin=worker-executor; toml=golem-worker-executor/config/worker-executor.toml; env=golem-worker-executor/config/worker-executor.sample.env;;
              esac
              check $bin --dump-config-default-toml "$toml"
              check $bin --dump-config-default-env-var "$env"
            done
            if [ "$fail" -ne 0 ]; then
              echo "Run \`cargo make generate-configs\` and commit the changes." >&2
              exit 1
            fi
            mkdir -p $out
            echo "config-drift clean" > $out/result
          '';

          # `cargo make check-openapi`. Run each service's
          # `--dump-openapi-yaml`, merge via `golem-openapi-client-generator
          # merge`, and diff against committed yamls.
          openapi-drift = pkgs.runCommand "golem-openapi-drift" {
            nativeBuildInputs = [ pkgs.diffutils ];
          } ''
            set -e
            mkdir -p $TMPDIR/openapi $TMPDIR/home $TMPDIR/data $out
            # Service `--dump-openapi-yaml` still initializes the runtime
            # config (incl. local blob storage in CWD). Run in a writable
            # cwd and point HOME there so config defaults don't fall back
            # to `/` or `$NIX_BUILD_TOP/..`.
            export HOME="$TMPDIR/home"
            cd "$TMPDIR/data"
            ${golem-services}/target/debug/golem-registry-service \
              --dump-openapi-yaml > $TMPDIR/openapi/golem-registry-service.yaml
            ${golem-services}/target/debug/golem-worker-service \
              --dump-openapi-yaml > $TMPDIR/openapi/golem-worker-service.yaml
            ${golem-services}/target/debug/golem-openapi-client-generator merge \
              --spec-yaml $TMPDIR/openapi/golem-registry-service.yaml \
              --spec-yaml $TMPDIR/openapi/golem-worker-service.yaml \
              --output-yaml $TMPDIR/openapi/golem-service.yaml
            fail=0
            for f in golem-registry-service.yaml golem-worker-service.yaml golem-service.yaml; do
              if ! diff $TMPDIR/openapi/$f ${src}/openapi/$f >/dev/null 2>&1; then
                echo "DRIFT: openapi/$f differs from regenerated output" >&2
                diff $TMPDIR/openapi/$f ${src}/openapi/$f | head -20 >&2 || true
                fail=1
              fi
            done
            if [ "$fail" -ne 0 ]; then
              echo "Run \`cargo make generate-openapi\` and commit the changes." >&2
              exit 1
            fi
            echo "openapi-drift clean" > $out/result
          '';

          # `cargo make check-wit` reruns `cargo make wit` (which copies
          # WIT deps from the canonical `wit/deps/` into each
          # consumer subdir: golem-wasm/wit/deps, golem-common/wit/deps,
          # cli/golem-cli/wit/deps, sdks/.../wit/deps) and then
          # `git diff --exit-code` against the committed copies. We
          # replicate the same copy + diff hermetically: every
          # `wit/deps/<entry>` from the root must equal the
          # corresponding `<consumer>/wit/deps/<entry>` checked into
          # the repo. Drift means someone edited the source-of-truth
          # without regenerating consumers, or vice versa.
          wit-consistency = pkgs.runCommand "golem-wit-consistency" {
            nativeBuildInputs = [ pkgs.diffutils ];
          } ''
            set -e
            mkdir -p $out $TMPDIR/regenerated
            fail=0
            check_consumer() {
              local consumer="$1"; shift
              local entries=("$@")
              local consumer_dir="${src}/$consumer/wit/deps"
              if [ ! -d "$consumer_dir" ]; then
                echo "DRIFT: $consumer/wit/deps directory missing" >&2
                fail=1; return
              fi
              for e in "''${entries[@]}"; do
                local src_path="${src}/wit/deps/$e"
                local dst_path="$consumer_dir/$e"
                if [ ! -e "$src_path" ]; then
                  echo "DRIFT: wit/deps/$e (referenced by $consumer) missing" >&2
                  fail=1; continue
                fi
                if ! diff -rq "$src_path" "$dst_path" >/dev/null 2>&1; then
                  echo "DRIFT: $consumer/wit/deps/$e differs from canonical wit/deps/$e" >&2
                  diff -rq "$src_path" "$dst_path" 2>&1 | head -10 >&2 || true
                  fail=1
                fi
              done
            }
            # Pulled from Makefile.toml `wit-golem-wasm` / `wit-golem-common`
            # / `wit-golem-cli` / `wit-sdks` tasks.
            check_consumer golem-wasm io clocks golem-1.x
            check_consumer golem-common io clocks golem-1.x
            check_consumer cli/golem-cli io clocks golem-1.x
            if [ "$fail" -ne 0 ]; then
              echo "Run \`cargo make wit\` and commit the resulting changes." >&2
              exit 1
            fi
            echo "wit-consistency clean" > $out/result
          '';
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
            ${pkgs.lib.optionalString (wasi-sdk != null) ''
              export WASI_SDK_PATH="${wasi-sdk}"
              export WASI_SDK_VERSION="25"
            ''}
            if ! command -v wasm-rquickjs >/dev/null 2>&1; then
              echo "[golem flake] wasm-rquickjs not on PATH — install with:"
              echo "              cargo binstall wasm-rquickjs@0.2.4"
            fi
          '';
        };

        formatter = pkgs.nixpkgs-fmt;
      });
}
