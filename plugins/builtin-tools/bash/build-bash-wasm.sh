#!/bin/sh
# Builds plugins/builtin-tools/bash.wasm reproducibly: the same bytes from any checkout path,
# target directory, machine or operating system. `cargo make build-builtin-tools` runs this.
#
#   build-bash-wasm.sh
#
# The build runs in a Linux x86-64 container (reproducible/Dockerfile) with the toolchain
# rust-toolchain.toml pins, so it needs Docker; on another platform Docker emulates x86-64.
# Inside it, the checkout is always /golem and the target directory /target, which is what makes
# the output independent of where the checkout is. The Docker volumes it creates only cache
# downloads and build output.
#
# GOLEM_BASH_WASM_TARGET_VOLUME names the volume that holds the target directory (default
# golem-bash-wasm-target). GOLEM_BASH_WASM_DOCKER_ARGS adds arguments to `docker run`, for
# example a mount for a local git mirror; neither changes the output.
set -eu

here=$(cd "$(dirname "$0")" && pwd -P)
repo=$(cd "$here/../../.." && pwd -P)
image=golem-bash-wasm-builder
target_volume=${GOLEM_BASH_WASM_TARGET_VOLUME:-golem-bash-wasm-target}

docker build --platform linux/amd64 --quiet --tag "$image" "$here/reproducible" >/dev/null

out=$(mktemp -d)
trap 'rm -rf "$out"' EXIT INT TERM
# shellcheck disable=SC2086 # GOLEM_BASH_WASM_DOCKER_ARGS is a list of arguments.
docker run --rm --platform linux/amd64 \
  --volume "$repo:/golem:ro" \
  --volume "$out:/out" \
  --volume "$target_volume:/target" \
  --volume golem-bash-wasm-registry:/usr/local/cargo/registry \
  --volume golem-bash-wasm-git:/usr/local/cargo/git \
  ${GOLEM_BASH_WASM_DOCKER_ARGS:-} \
  "$image" sh /golem/plugins/builtin-tools/bash/reproducible/build-in-container.sh

cp "$out/bash.wasm" "$repo/plugins/builtin-tools/bash.wasm"
if command -v sha256sum >/dev/null; then
  sha256sum "$repo/plugins/builtin-tools/bash.wasm"
else
  shasum -a 256 "$repo/plugins/builtin-tools/bash.wasm"
fi
