#!/bin/sh
# Runs in the image ./Dockerfile describes (see ../build-bash-wasm.sh): the golem checkout is at
# /golem, read-only; the target directory is /target; bash.wasm is written to /out.
set -eu

cd /golem/plugins/builtin-tools/bash
channel=$(sed -n 's/^channel = "\(.*\)"/\1/p' rust-toolchain.toml)
rustc -vV | grep -q "^release: $channel$" || {
  echo "error: the image's rustc is not $channel, which rust-toolchain.toml pins" >&2
  exit 1
}

# Source paths end up in panic messages. The checkout is always /golem here; the dependencies and
# the toolchain's library sources are named as they would be on any machine: CARGO_HOME as /cargo
# and the library as /rustc/<commit>, where the prebuilt standard library already says it lives.
sysroot=$(rustc --print sysroot)
commit=$(rustc -vV | sed -n 's/^commit-hash: //p')
separator=$(printf '\037')
flags="--remap-path-prefix=$CARGO_HOME=/cargo"
flags="$flags$separator--remap-path-prefix=$sysroot/lib/rustlib/src/rust=/rustc/$commit"

# The same build golem.yaml runs, with the profile Cargo.toml pins.
CARGO_ENCODED_RUSTFLAGS="$flags" CARGO_TARGET_DIR=/target \
  cargo build --locked --release --target wasm32-wasip2 --package golem-bash-tool --lib

# The component is named as golem-cli names it (golem:bash).
wasm-tools metadata add --name golem:bash /target/wasm32-wasip2/release/golem_bash.wasm \
  -o /out/bash.wasm
wasm-tools validate --features all /out/bash.wasm
