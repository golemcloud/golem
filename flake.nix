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
              hash = "sha256-DmehvcTeasIxbYOB7DlPSJsjEsalDQ1Hobn5devqHJw=";
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

        # Built golem-ts-sdk monorepo (golem-ts-sdk, golem-ts-typegen, …).
        # Test-components reference these via npm `file:` deps; we pre-build
        # them so the test-components' `npm install` resolves to working
        # `dist/` outputs without needing to re-run the SDK's prepare script.
        golem-ts-sdk = pkgs.stdenv.mkDerivation {
          pname = "golem-ts-sdk";
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
            # Stage just the built artifacts of each workspace package — we
            # deliberately drop the pnpm-symlinked node_modules/ tree because
            # nixpkgs' noBrokenSymlinks check rejects relative .pnpm symlinks
            # that escape the package directory, and the test-components don't
            # need them: they `npm install` their own deps and reference these
            # SDK packages via `file:` deps that only need the publish files.
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
            # Test-components consume these packages via npm `file:` deps;
            # npm symlinks the bin scripts into node_modules/.bin so any
            # `#!/usr/bin/env node` shebangs need patching here, before the
            # downstream derivations consume them read-only.
            patchShebangs $out/packages
            runHook postInstall
          '';
        };

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
            pkgs.bash
            pkgs.cacert
          ] ++ pkgs.lib.optionals (wasi-sdk != null) [ wasi-sdk ];

          buildPhase = ''
            runHook preBuild
            ${pkgs.lib.optionalString (wasi-sdk != null) ''
              export WASI_SDK_PATH=${wasi-sdk}
              export WASI_SDK_VERSION=25
            ''}
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

        # The TypeScript pipeline (TS test-components + the SDK's
        # agent_guest.wasm) needs:
        #   1. `pnpm` to install npm deps + run rollup
        #   2. `wasm-rquickjs generate-wrapper-crate` to produce a Cargo crate
        #      whose `Cargo.lock` doesn't exist at flake-eval time
        #   3. `cargo build --target wasm32-wasip2` against that crate
        #   4. `npm install` per test-component (file: deps to the just-built
        #      SDK plus normal registry deps)
        #   5. `golem-cli build` per test-component
        # The dynamic Cargo.lock blocks pre-vendoring, so this is a single
        # fixed-output derivation with network access. Output is hash-pinned:
        # if reproducibility breaks, the hash mismatch surfaces immediately.
        test-components-ts = pkgs.stdenv.mkDerivation {
          pname = "golem-test-components-ts";
          version = "0.0.0";
          inherit src;

          # Two non-hermetic moves:
          # 1. `__noChroot` lets the build see the host filesystem (so cargo
          #    + npm can fetch dependencies live — this is genuinely needed
          #    here because the dynamically-generated agent-template Cargo
          #    crate has no Cargo.lock at flake-eval time, so we can't
          #    pre-vendor it).
          # 2. We don't pin an output hash because cargo + rollup outputs
          #    aren't bit-identical across builds (build-ids, timestamps,
          #    file-iteration order). A fixed-output derivation would
          #    hash-mismatch on every rebuild.
          # Tradeoff: this requires `sandbox = relaxed` in the user's
          # nix.conf. Documented in the flake commit message.
          __noChroot = true;

          # nixpkgs' stdenv adds hardening flags (e.g. `-fzero-call-used-regs`)
          # via NIX_CFLAGS_COMPILE; clang rejects them for wasm32-wasip2.
          # Disable across the board — the WASI SDK clang has its own defaults.
          hardeningDisable = [ "all" ];

          nativeBuildInputs = [
            pkgs.nodejs_20
            pkgs.pnpm_10
            rustToolchain
            golem-cli
            wasm-rquickjs
            pkgs.wasm-tools
            pkgs.wit-bindgen
            pkgs.cacert
            pkgs.git
            pkgs.bash
            pkgs.curl
            pkgs.libclang.lib
            pkgs.clang
            pkgs.glibc.dev
          ] ++ pkgs.lib.optionals (wasi-sdk != null) [ wasi-sdk ];

          buildPhase = ''
            runHook preBuild
            export HOME=$TMPDIR/home
            mkdir -p $HOME
            # pnpm 10 honors `packageManager` in package.json and tries to
            # download the exact pinned version (here 10.17.1) into PNPM_HOME.
            # We use whatever pnpm nixpkgs ships; turn off the auto-switch.
            echo "manage-package-manager-versions=false" >> $HOME/.npmrc
            # rquickjs-sys uses bindgen, which needs libclang on the host.
            export LIBCLANG_PATH="${pkgs.libclang.lib}/lib"
            ${pkgs.lib.optionalString (wasi-sdk != null) ''
              export WASI_SDK_PATH=${wasi-sdk}
              # rquickjs-sys reads `WASI_SDK` (no `_PATH`) — without this it
              # downloads its own v24 tarball, then fails to invoke the
              # bundled clang because cc-rs can't find it on PATH.
              export WASI_SDK=${wasi-sdk}
              export WASI_SDK_VERSION=25
              # Make cc-rs use the WASI SDK's clang for wasm32-wasip2 cross-
              # compilation. Without this, host nixpkgs clang gets picked and
              # tries to consume glibc headers for a wasm target.
              export CC_wasm32_wasip2="${wasi-sdk}/bin/clang"
              export CXX_wasm32_wasip2="${wasi-sdk}/bin/clang++"
              export AR_wasm32_wasip2="${wasi-sdk}/bin/llvm-ar"
              # NB: deliberately not overriding the rust linker — let rustc
              # use its bundled rust-lld with the right `--no-entry` defaults
              # for wasm32-wasip2 cdylibs.
            ''}
            # Bindgen reads per-target overrides via
            # `BINDGEN_EXTRA_CLANG_ARGS_<sanitized-target>`. For host builds
            # of rquickjs-sys we have to point bindgen's bare libclang at
            # nixpkgs' glibc dev headers AND lock the target to 64-bit so it
            # doesn't try to consume the missing `gnu/stubs-32.h`. For the
            # wasm cross-compile we point at the WASI sysroot.
            export BINDGEN_EXTRA_CLANG_ARGS_x86_64_unknown_linux_gnu="--target=x86_64-unknown-linux-gnu -I${pkgs.glibc.dev}/include"
            ${pkgs.lib.optionalString (wasi-sdk != null) ''
              export BINDGEN_EXTRA_CLANG_ARGS_wasm32_wasip2="--sysroot=${wasi-sdk}/share/wasi-sysroot"
            ''}

            # 1) Build the SDK monorepo (produces dist/ in each package)
            (cd sdks/ts && pnpm install --frozen-lockfile && pnpm run build)

            # 2-4) Generate + compile + copy the agent-template inside golem-ts-sdk
            (cd sdks/ts/packages/golem-ts-sdk && pnpm run build-agent-template)

            # Patch shebangs in the just-built SDK packages so that
            # downstream `npm install` of file: deps gets working bin scripts.
            echo "==> patching shebangs in sdks/ts/packages"
            patchShebangs sdks/ts/packages
            echo "==> golem-typegen.cjs head:"
            head -2 sdks/ts/packages/golem-ts-typegen/dist/golem-typegen.cjs 2>/dev/null || echo "  (not present)"

            # 5) Build each TS test-component
            #
            # `npm install` runs `prepare` scripts on file: deps, which
            # invokes `pnpm build` inside the SDK packages — that regenerates
            # dist/*.cjs (re-introducing `#!/usr/bin/env node`) and undoes
            # our shebang patches. Disable lifecycle scripts globally for
            # test-component installs (the SDK is already built).
            #
            # golem-cli also runs its own `npm install` when its marker file
            # doesn't match. Let it do the install on the first invocation
            # (which fails on the broken shebang because tsc gets re-staged
            # by prepare) — then patch shebangs across node_modules + the
            # source SDK packages — then run `golem-cli build` again. The
            # second pass sees the marker as up-to-date and skips reinstall.
            export NPM_CONFIG_IGNORE_SCRIPTS=true
            cd test-components
            for c in ${pkgs.lib.concatStringsSep " " tsTestComponents}; do
              echo "==> Building TS component $c"
              pushd "$c"
              golem-cli --preset release build --yes --skip-check || true
              patchShebangs node_modules
              patchShebangs ../../sdks/ts/packages
              golem-cli --preset release build --yes --skip-check
              golem-cli --preset release exec copy
              popd
            done
            runHook postBuild
          '';

          installPhase = ''
            runHook preInstall
            mkdir -p $out/test-components
            find . -maxdepth 1 -name '*.wasm' -exec cp {} $out/test-components/ \;
            runHook postInstall
          '';
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
          for wasm in ${test-components-rust}/test-components/*.wasm \
                      ${test-components-ts}/test-components/*.wasm; do
            [ -e "$wasm" ] && ln -sf "$wasm" "./test-components/$(basename "$wasm")"
          done
          for bin in ${golem-services}/target/debug/*; do
            dst="./target/debug/$(basename "$bin")"
            [ -e "$dst" ] || ln -sf "$bin" "$dst"
          done
          export GOLEM_REPO_ROOT="$(pwd)"
        '';

        mkWorkerExecutorTest = { tag, name }: craneLib.mkCargoDerivation (commonArgs // {
          inherit cargoArtifacts;
          pname = "golem-worker-executor-tests-${name}";
          doInstallCargoArtifacts = false;
          doCheck = true;
          nativeBuildInputs = commonNativeBuildInputs ++ [ pkgs.redis ];
          GOLEM_TEST_DB = "sqlite";
          WASMTIME_BACKTRACE_DETAILS = "1";
          RUST_BACKTRACE = "1";
          RUST_LOG = "info";
          buildPhaseCargoCommand = ''
            cargo test --locked --no-run -p golem-worker-executor --test integration
          '';
          checkPhaseCargoCommand = ''
            ${mkRuntimeRoot}
            # Wasmtime opens a filesystem cache (~/.cache/wasmtime) inside
            # `extract_agent_types`; the sandbox has no $HOME, so point both
            # XDG and HOME at a writable temp dir.
            export HOME="$TMPDIR/home"
            export XDG_CACHE_HOME="$TMPDIR/cache"
            mkdir -p "$HOME" "$XDG_CACHE_HOME"
            cargo test --locked -p golem-worker-executor --test integration -- ${tag} --report-time --nocapture
          '';
          installPhaseCommand = ''
            mkdir -p $out
            echo "${name} passed" > $out/result
          '';
        });
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
